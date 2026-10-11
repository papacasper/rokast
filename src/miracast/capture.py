"""Screen capture -> H.264(+audio) -> RTP/MPEG-TS, parameterised by the negotiated WFD format."""
import os
import subprocess

LEVEL_FLAG = {"3.1": "3.1", "3.2": "3.2", "4.0": "4.0", "4.1": "4.1", "4.2": "4.2"}


def default_monitor():
    try:
        sink = subprocess.run(["pactl", "get-default-sink"], capture_output=True, text=True, check=True).stdout.strip()
        return sink + ".monitor" if sink else None
    except (OSError, subprocess.CalledProcessError):
        return None


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


def build_cmd(n, sink_ip, server_port, display=":0", encoder="h264_nvenc", bitrate="8M", audio=True, monitor=None):
    """Pure function: Negotiated + target -> ffmpeg argv. Unit-tested."""
    gop = n.fps  # 1s GOP: sink can resync/IDR-request cheaply
    display = display.split(".")[0]  # ":0.0" (inherited $DISPLAY) -> ":0"; the screen suffix is appended below
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin",
           "-fflags", "nobuffer", "-flags", "low_delay"]
    if encoder == "h264_vaapi":
        cmd += ["-vaapi_device", VAAPI_DEVICE]
    cmd += ["-f", "x11grab", "-framerate", str(n.fps), "-draw_mouse", "1", "-i", f"{display}.0+0,0"]
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
    cmd += ["-profile:v", "baseline" if n.profile == "baseline" else "high",
            "-level", LEVEL_FLAG.get(n.level, "4.0"), "-g", str(gop), "-keyint_min", str(gop)]
    if use_audio:
        acodec = ["-c:a", "aac", "-b:a", "160k"] if n.audio == "AAC" else ["-c:a", "pcm_s16be"]
        cmd += ["-map", "1:a", *acodec, "-ar", "48000", "-ac", "2", "-af", "aresample=async=1:first_pts=0"]
    cmd += ["-muxdelay", "0", "-muxpreload", "0", "-flush_packets", "1",
            "-mpegts_flags", "+resend_headers", "-pat_period", "0.2",
            "-f", "rtp_mpegts", f"rtp://{sink_ip}:{n.rtp_port}?localport={server_port}&pkt_size=1316"]
    return cmd


def start(cmd, display=":0", xauth=None):
    env = dict(os.environ, DISPLAY=display, XAUTHORITY=xauth or os.path.expanduser("~/.Xauthority"))
    return subprocess.Popen(cmd, env=env)
