"""rokast selftest + hardware report: no TV or Wi-Fi Direct needed.

selftest runs the real capture + encode pipeline (capture.build_cmd, same encoder pick, same X11/Wayland path) into an
MPEG-TS file for a few seconds, then checks the file with ffprobe. That makes capture testable on any machine, VM or
live USB (GNOME/KDE portals, VAAPI...). report = what a hardware report issue needs.
"""
import json
import os
import platform
import shutil
import subprocess
import tempfile

import capture
import wayland
import wfd

MODES = {"1080p60": (1920, 1080, 60), "1080p30": (1920, 1080, 30), "720p60": (1280, 720, 60),
         "720p30": (1280, 720, 30), "480p60": (640, 480, 60)}
MIN_FRAMES = 0.8   # share of fps * seconds that must arrive (x11grab/portal can drop a few at start)


def negotiated(res):
    """What a Roku would most likely negotiate: the first known entry of RES ("1080p30,720p60,..."), High profile
    (what the Living Room Roku negotiates; baseline is only used when a TV offers it), level 4.2 for 1080p60 else 4.0."""
    for r in (x.strip() for x in res.split(",")):
        if r in MODES:
            w, h, fps = MODES[r]
            return wfd.Negotiated(w, h, fps, "high", "4.2" if (h, fps) == (1080, 60) else "4.0", "AAC", 0)
    raise RuntimeError(f"RES has no known mode (got {res!r}; known: {', '.join(MODES)})")


def probe_cmd(path):
    return ["ffprobe", "-v", "error", "-count_frames", "-show_entries",
            "stream=codec_type,codec_name,width,height,nb_read_frames", "-of", "json", path]


def check(probe, n, seconds, audio):
    """Pure: ffprobe JSON + expectations -> list of problems (empty = pass). Unit-tested."""
    streams = probe.get("streams", [])
    video = [s for s in streams if s.get("codec_type") == "video"]
    problems = []
    if not video:
        return ["no video stream in the output"]
    v = video[0]
    if v.get("codec_name") != "h264":
        problems.append(f"video codec {v.get('codec_name')}, expected h264")
    if (v.get("width"), v.get("height")) != (n.width, n.height):
        problems.append(f"video {v.get('width')}x{v.get('height')}, expected {n.width}x{n.height}")
    frames, want = int(v.get("nb_read_frames") or 0), int(n.fps * seconds * MIN_FRAMES)
    if frames < want:
        problems.append(f"only {frames} frames in {seconds}s, expected at least {want} ({n.fps} fps)")
    has_audio = any(s.get("codec_type") == "audio" for s in streams)
    if audio and not has_audio:
        problems.append("no audio stream (AUDIO=1; is PulseAudio/PipeWire-pulse running? `pactl get-default-sink`)")
    return problems


def summary(probe):
    parts = []
    for s in probe.get("streams", []):
        if s.get("codec_type") == "video":
            parts.append(f"{s.get('codec_name')} {s.get('width')}x{s.get('height')} {s.get('nb_read_frames')} frames")
        elif s.get("codec_type") == "audio":
            parts.append(f"{s.get('codec_name')} audio")
    return ", ".join(parts)


def run(conf, mode, seconds=5, out=None, log=lambda m: print(m, flush=True)):
    """Capture + encode `seconds` into `out` (a temp file, deleted after, if None). Returns True on pass."""
    if not shutil.which("ffprobe"):
        raise RuntimeError("ffprobe not found (it comes with ffmpeg)")
    n = negotiated(conf["RES"])
    encoder = capture.pick_encoder(conf["ENCODER"])
    monitor = capture.default_monitor() if conf["AUDIO"] == "1" else None
    keep = out is not None
    if not keep:
        fd, out = tempfile.mkstemp(prefix="rokast-selftest-", suffix=".ts")
        os.close(fd)
    log(f"selftest: {mode} capture{' of ' + conf['MONITOR'] if conf['MONITOR'] else ''}, {encoder}, {n.width}x{n.height}@{n.fps}, "
        f"audio {'from ' + monitor if monitor else 'off'}, {seconds}s -> {out}")
    cast = None
    try:
        if mode == "wayland":
            cast = wayland.ScreenCast()
            cast.open()
        geometry = None if cast else capture.monitor_geometry(capture.x11_monitors(conf["DISPLAY"]), conf["MONITOR"])
        cmd = capture.build_cmd(n, None, None, conf["DISPLAY"], encoder, conf["BITRATE"], bool(monitor), monitor,
                                source="raw" if cast else "x11", out=out, seconds=seconds, geometry=geometry)
        feeder = wayland.gst_cmd(cast.fd, cast.node, n.width, n.height, n.fps) if cast else None
        pipe = capture.start(cmd, conf["DISPLAY"], feeder=feeder, fd=cast.fd if cast else None)
        ff = pipe.procs[-1]
        try:
            code = ff.wait(timeout=seconds + 30)
        except subprocess.TimeoutExpired:
            code = "timeout"
        pipe.terminate()
        if code != 0:
            log(f"selftest: FAIL ffmpeg exited with {code} (its messages are above)")
            if encoder == "h264_vaapi":
                log("selftest: hint: VAAPI needs your GPU's VA-API driver (intel-media-driver for recent Intel, "
                    "Mesa VA-API for AMD); ENCODER=libx264 works everywhere")
            return False
        r = subprocess.run(probe_cmd(out), capture_output=True, text=True, timeout=60)
        if r.returncode != 0:
            log(f"selftest: FAIL ffprobe could not read the output: {r.stderr.strip()}")
            return False
        probe = json.loads(r.stdout)
        problems = check(probe, n, seconds, bool(monitor))
        for p in problems:
            log(f"selftest: FAIL {p}")
        if not problems:
            log(f"selftest: ok {summary(probe)}")
        return not problems
    finally:
        if cast:
            cast.close()
        if not keep and os.path.exists(out):
            os.remove(out)


def _run(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def os_release(path="/etc/os-release"):
    try:
        for ln in open(path):
            if ln.startswith("PRETTY_NAME="):
                return ln.split("=", 1)[1].strip().strip('"')
    except OSError:
        pass
    return "unknown"


def wifi_chips(sys_net="/sys/class/net"):
    """[(iface, driver)] for wireless interfaces."""
    out = []
    try:
        names = sorted(os.listdir(sys_net))
    except OSError:
        return out
    for name in names:
        if os.path.isdir(os.path.join(sys_net, name, "wireless")):
            drv = os.path.join(sys_net, name, "device", "driver")
            out.append((name, os.path.basename(os.path.realpath(drv)) if os.path.exists(drv) else "?"))
    return out


def system_info(env=os.environ):
    """[(label, value)] for a hardware report."""
    pci = _run(["lspci"]).splitlines()
    gpus = [ln.split(": ", 1)[-1] for ln in pci if any(k in ln for k in ("VGA", "3D controller", "Display controller"))]
    nets = [ln.split(": ", 1)[-1] for ln in pci if "Network controller" in ln]
    return [
        ("distro", os_release()),
        ("kernel", platform.release()),
        ("desktop", env.get("XDG_CURRENT_DESKTOP") or "unknown"),
        ("session", env.get("XDG_SESSION_TYPE") or ("wayland" if wayland.is_wayland(env) else "x11?")),
        ("gpu", "; ".join(gpus) or "unknown (lspci missing?)"),
        ("wifi", "; ".join(nets) or "unknown"),
        ("wifi driver", ", ".join(f"{i}: {d}" for i, d in wifi_chips()) or "none"),
        ("ffmpeg", (_run(["ffmpeg", "-version"]).splitlines() or ["missing"])[0]),
    ]
