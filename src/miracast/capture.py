"""Screen capture -> H.264(+audio) -> RTP/MPEG-TS, parameterised by the negotiated WFD format."""
import os
import re
import subprocess

LEVEL_FLAG = {"3.1": "3.1", "3.2": "3.2", "4.0": "4.0", "4.1": "4.1", "4.2": "4.2"}


def default_monitor():
    try:
        sink = subprocess.run(["pactl", "get-default-sink"], capture_output=True, text=True, check=True).stdout.strip()
        return sink + ".monitor" if sink else None
    except (OSError, subprocess.CalledProcessError):
        return None


MONITOR_RE = re.compile(r"^\s*\d+:\s+([+*]*)(\S+)\s+(\d+)/\d+x(\d+)/\d+\+(\d+)\+(\d+)")


def parse_monitors(xrandr_output):
    """Pure: `xrandr --listmonitors` -> [{name, width, height, x, y, primary}]. Unit-tested."""
    out = []
    for ln in xrandr_output.splitlines():
        m = MONITOR_RE.match(ln)
        if m:
            flags, name, w, h, x, y = m.groups()
            out.append({"name": name, "width": int(w), "height": int(h), "x": int(x), "y": int(y),
                        "primary": "*" in flags})
    return out


def monitor_geometry(xrandr_output, name=""):
    """Pure: the monitor to capture. `name` empty = the primary one (else the first). None when xrandr listed no
    monitors (capture the whole X screen, as before). Unknown name -> RuntimeError naming the choices."""
    mons = parse_monitors(xrandr_output)
    if not mons:
        return None
    if not name:
        return next((m for m in mons if m["primary"]), mons[0])
    for m in mons:
        if m["name"] == name:
            return m
    raise RuntimeError(f"MONITOR={name!r} not found; monitors: {', '.join(m['name'] for m in mons)}")


def x11_monitors(display=":0", xauth=None):
    """`xrandr --listmonitors` output for `display` ("" if xrandr is missing or fails)."""
    env = dict(os.environ, DISPLAY=display, XAUTHORITY=xauth or os.environ.get("XAUTHORITY") or os.path.expanduser("~/.Xauthority"))
    try:
        return subprocess.run(["xrandr", "--listmonitors"], capture_output=True, text=True, env=env, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


ENCODERS = ("h264_nvenc", "h264_vaapi", "libx264")   # auto-detect order: NVIDIA, Intel/AMD (VAAPI), software
VAAPI_DEVICE = "/dev/dri/renderD128"


def _probe_args(encoder):
    if encoder == "h264_vaapi":
        return ["-vaapi_device", VAAPI_DEVICE, "-f", "lavfi", "-i", "testsrc2=s=640x360:d=0.2",
                "-vf", "format=nv12,hwupload", "-c:v", encoder]
    return ["-f", "lavfi", "-i", "testsrc2=s=640x360:d=0.2", "-c:v", encoder]


def pick_encoder(wanted="auto"):
    """Return the encoder to use: `wanted` itself, or for "auto" the first one that actually encodes here
    (an encoder being *listed* by ffmpeg does not mean the GPU/driver is present)."""
    if wanted != "auto":
        return wanted
    for enc in ENCODERS:
        r = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", *_probe_args(enc), "-f", "null", "-"],
                           capture_output=True, timeout=30)
        if r.returncode == 0:
            return enc
    raise RuntimeError("no working H.264 encoder (install ffmpeg with libx264)")


def build_cmd(n, sink_ip, server_port, display=":0", encoder="h264_nvenc", bitrate="8M", audio=True, monitor=None,
              source="x11", out=None, seconds=None, geometry=None):
    """Pure function: Negotiated + target -> ffmpeg argv. Unit-tested.

    source "x11": x11grab of `display`; "raw": I420 frames of exactly n.width x n.height @ n.fps on stdin
    (the Wayland path, see wayland.gst_cmd). `out`: write MPEG-TS to that file (for `seconds`) instead of RTP
    (rokast selftest: the same pipeline, minus the TV). `geometry`: x11grab only that monitor (monitor_geometry())."""
    gop = n.fps  # 1s GOP: sink can resync/IDR-request cheaply
    display = display.split(".")[0]  # ":0.0" (inherited $DISPLAY) -> ":0"; the screen suffix is appended below
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin",
           "-fflags", "nobuffer", "-flags", "low_delay"]
    if encoder == "h264_vaapi":
        cmd += ["-vaapi_device", VAAPI_DEVICE]
    if source == "raw":
        cmd += ["-f", "rawvideo", "-pix_fmt", "yuv420p", "-video_size", f"{n.width}x{n.height}",
                "-framerate", str(n.fps), "-i", "pipe:0"]
    else:
        g = geometry or {"x": 0, "y": 0}
        size = ["-video_size", f"{g['width']}x{g['height']}"] if geometry else []
        cmd += ["-f", "x11grab", "-framerate", str(n.fps), "-draw_mouse", "1", *size, "-i", f"{display}.0+{g['x']},{g['y']}"]
    use_audio = audio and monitor
    if use_audio:
        cmd += ["-thread_queue_size", "512", "-f", "pulse", "-i", monitor]
    cmd += ["-map", "0:v", "-vf",
            f"scale={n.width}:{n.height}:force_original_aspect_ratio=decrease,"
            f"pad={n.width}:{n.height}:(ow-iw)/2:(oh-ih)/2,"
            + ("format=nv12,hwupload" if encoder == "h264_vaapi" else "format=yuv420p")]
    if encoder == "h264_nvenc":
        cmd += ["-c:v", "h264_nvenc", "-preset", "p1", "-tune", "ll", "-rc", "cbr",
                "-b:v", bitrate, "-maxrate", bitrate, "-bufsize", "1M", "-bf", "0",
                "-rc-lookahead", "0", "-forced-idr", "1"]
    elif encoder == "h264_vaapi":
        cmd += ["-c:v", "h264_vaapi", "-rc_mode", "CBR", "-b:v", bitrate, "-maxrate", bitrate, "-bf", "0"]
    else:
        cmd += ["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
                "-b:v", bitrate, "-maxrate", bitrate, "-bufsize", "1M", "-bf", "0"]
    level = LEVEL_FLAG.get(n.level, "4.0")
    if encoder == "h264_vaapi":   # its names differ: "constrained_baseline", and level as level_idc ("4.0" breaks encoding)
        cmd += ["-profile:v", "constrained_baseline" if n.profile == "baseline" else "high",
                "-level", str(round(float(level) * 10))]
    else:
        cmd += ["-profile:v", "baseline" if n.profile == "baseline" else "high", "-level", level]
    cmd += ["-g", str(gop), "-keyint_min", str(gop)]
    if use_audio:
        acodec = ["-c:a", "aac", "-b:a", "160k"] if n.audio == "AAC" else ["-c:a", "pcm_s16be"]
        cmd += ["-map", "1:a", *acodec, "-ar", "48000", "-ac", "2", "-af", "aresample=async=1:first_pts=0"]
    cmd += ["-muxdelay", "0", "-muxpreload", "0", "-flush_packets", "1",
            "-mpegts_flags", "+resend_headers", "-pat_period", "0.2"]
    if out:
        cmd += (["-t", str(seconds)] if seconds else []) + ["-y", "-f", "mpegts", out]
    else:
        cmd += ["-f", "rtp_mpegts", f"rtp://{sink_ip}:{n.rtp_port}?localport={server_port}&pkt_size=1316"]
    return cmd


class Pipeline:
    """ffmpeg, optionally fed by a GStreamer PipeWire capture; poll()/terminate() like a Popen."""

    def __init__(self, procs):
        self.procs = procs

    def poll(self):
        codes = [p.poll() for p in self.procs]
        return next((c for c in codes if c is not None), None)

    def terminate(self):
        for p in reversed(self.procs):
            if p.poll() is None:
                p.terminate()


def start(cmd, display=":0", xauth=None, feeder=None, fd=None):
    """Start ffmpeg; with `feeder` (gst argv) its stdout becomes ffmpeg's stdin and `fd` is passed to it."""
    env = dict(os.environ, DISPLAY=display, XAUTHORITY=xauth or os.path.expanduser("~/.Xauthority"))
    if not feeder:
        return Pipeline([subprocess.Popen(cmd, env=env)])
    gst = subprocess.Popen(feeder, stdout=subprocess.PIPE, pass_fds=(fd,))
    ff = subprocess.Popen(cmd, env=env, stdin=gst.stdout)
    gst.stdout.close()   # ffmpeg owns the read end; gst gets SIGPIPE if ffmpeg dies
    return Pipeline([gst, ff])
