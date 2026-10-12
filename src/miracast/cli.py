"""rokast CLI: list [--json] | start [name] | name | config | doctor [--report] | selftest [--seconds N] [--out FILE]
| monitors [--json] | ui [--tray] | version [--check [--json]].  Foreground; Ctrl+C stops and tears the link down."""
import asyncio
import json
import os
import signal
import socket
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture  # noqa: E402
import p2p  # noqa: E402
import paths  # noqa: E402
import selftest  # noqa: E402
import update  # noqa: E402
import wayland  # noqa: E402
import wfd  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def load_conf():
    conf = {"TARGET": "", "BITRATE": "8M", "ENCODER": "auto", "AUDIO": "1",
            "DISPLAY": ":0", "RES": "1080p30,720p60,720p30", "NAME": "", "CAPTURE": "auto",
            "MONITOR": "", "CHECK_UPDATES": "1"}
    path, legacy = paths.config_file()
    if legacy:
        paths.migrate_config(path, legacy)
    conf["_file"] = path
    if os.path.exists(path):
        for ln in open(path):
            ln = ln.split("#")[0].strip()
            if "=" in ln:
                k, v = ln.split("=", 1)
                conf[k.strip()] = v.strip()
    conf.update({k: os.environ[k] for k in conf if k in os.environ})
    conf["NAME"] = clean_name(conf["NAME"]) or default_name()
    return conf


def clean_name(name):
    """The name the TV shows: printable, at most 32 bytes (the Wi-Fi Direct device-name limit)."""
    name = "".join(c for c in name if c.isprintable()).strip()
    while len(name.encode()) > 32:
        name = name[:-1]
    return name.strip()


def default_name():
    """This computer's name: the pretty hostname (e.g. "Casper's ThinkPad"), else the hostname."""
    try:
        pretty = subprocess.run(["hostnamectl", "--pretty"], capture_output=True, text=True, timeout=5).stdout
    except (OSError, subprocess.SubprocessError):
        pretty = ""
    return clean_name(pretty) or clean_name(socket.gethostname()) or "Linux"


def capture_mode(conf):
    mode = conf["CAPTURE"]
    if mode == "auto":
        return "wayland" if wayland.is_wayland() else "x11"
    if mode not in ("x11", "wayland"):
        raise RuntimeError(f"CAPTURE must be auto, x11 or wayland (got {mode!r})")
    return mode


def capture_geometry(conf, mode):
    """X11: the MONITOR to capture (empty = primary), or None for the whole screen. Wayland: the portal picks."""
    if mode != "x11":
        return None
    return capture.monitor_geometry(capture.x11_monitors(conf["DISPLAY"]), conf["MONITOR"])


def status(state, detail=""):
    """Machine-readable progress line (the UI keys off these):
    scanning, sharing (Wayland screen picker), joining, waiting, streaming, stopped, error."""
    print(f"status: {state}" + (f" {detail}" if detail else ""), flush=True)


def cmd_list(fmt=None):
    rokus = p2p.lan_rokus()
    if fmt == "--json":
        print(json.dumps(rokus))
        return
    for r in rokus:
        print(f"{r['name']:<24} {r['ip']:<15} {r['model']}  p2p={r['p2p_mac']}")


async def stream(conf, iface, cast=None):
    proc = {"p": None}

    async def on_play(n, ip, server_port):
        cmd = capture.build_cmd(n, ip, server_port, conf["DISPLAY"], conf["ENCODER"], conf["BITRATE"],
                                conf["AUDIO"] == "1" and n.audio == "AAC", capture.default_monitor(),
                                source="raw" if cast else "x11", geometry=conf.get("_geometry"))
        feeder = wayland.gst_cmd(cast.fd, cast.node, n.width, n.height, n.fps) if cast else None
        if feeder:
            print("gst:", " ".join(feeder), flush=True)
        print("ffmpeg:", " ".join(cmd), flush=True)
        proc["p"] = capture.start(cmd, conf["DISPLAY"], feeder=feeder, fd=cast.fd if cast else None)
        status("streaming", f"{n.width}x{n.height}@{n.fps} {n.audio if conf['AUDIO'] == '1' else 'no audio'}")

    async def on_stop():
        if proc["p"] and proc["p"].poll() is None:
            proc["p"].terminate()

    src = wfd.Source(None, [p.strip() for p in conf["RES"].split(",")])
    src.on_play, src.on_stop = on_play, on_stop
    serving = asyncio.create_task(src.serve(device=iface))   # must listen BEFORE the lease: the TV connects ~0.5 s after it
    await src.listening.wait()
    local_ip = await asyncio.to_thread(p2p.dhcp, iface)
    print(f"link up: {iface} {local_ip}, waiting for the TV to connect", flush=True)
    status("waiting")
    try:
        await asyncio.wait_for(asyncio.shield(src.playing.wait()), 20)
    except asyncio.TimeoutError:
        serving.cancel()
        raise RuntimeError("the TV joined but never opened RTSP to us (screen-mirroring prompt declined?)") from None
    await serving


def cmd_start(name=None):
    conf = load_conf()
    name = name or conf["TARGET"]
    if not name:
        status("error", "no TV selected")
        sys.exit(f"no target: `rokast start \"<name>\"` or set TARGET in {conf['_file']} (see `rokast list`)")
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    dev = cast = None
    try:
        status("scanning", name)
        target = p2p.pick(p2p.lan_rokus(), name)
        print(f"target: {target['name']} ({target['ip']}, p2p {target['p2p_mac']})", flush=True)
        conf["ENCODER"] = capture.pick_encoder(conf["ENCODER"])   # probe before joining: the TV's RTSP window is short
        print(f"encoder: {conf['ENCODER']}", flush=True)
        mode = capture_mode(conf)
        conf["_geometry"] = capture_geometry(conf, mode)   # unknown MONITOR fails here, before joining
        g = conf["_geometry"]
        print(f"capture: {mode}" + (f" monitor {g['name']} {g['width']}x{g['height']}+{g['x']}+{g['y']}" if g else "")
              + f", name on TV: {conf['NAME']}", flush=True)
        if mode == "wayland":   # the portal may ask the user to pick a screen: do it before joining
            status("sharing")
            cast = wayland.ScreenCast()
            cast.open()
            print(f"pipewire: node {cast.node}", flush=True)
        dev = p2p.p2p_dev_iface()
        status("joining", target["name"])
        print("joining the TV's Wi-Fi Direct group...", flush=True)
        iface = p2p.connect(dev, target, conf["NAME"])
        asyncio.run(stream(conf, iface, cast))
    except KeyboardInterrupt:
        print("stopping", flush=True)
    except (RuntimeError, OSError, subprocess.SubprocessError) as e:
        status("error", str(e))
        sys.exit(f"error: {e}")
    finally:
        if dev:
            p2p.disconnect(dev)
        if cast:
            cast.close()
    status("stopped")


def cmd_doctor(report=False):
    """Checks everything `start` needs. --report: plus system info and a selftest, for a hardware report issue."""
    import shutil
    conf = load_conf()
    if report:
        print(f"rokast {open(os.path.join(ROOT, 'VERSION')).read().strip()}")
        for k, v in selftest.system_info():
            print(f"{k + ':':<13}{v}")
    ok = True

    def chk(label, cond, extra=""):
        nonlocal ok
        print(("ok    " if cond else "FAIL  ") + label + (f"  ({extra})" if extra and not cond else ""))
        ok = ok and bool(cond)

    for t in ("ffmpeg", "wpa_cli", "dhcpcd", "iw"):
        chk(f"{t} installed", shutil.which(t))
    mode = capture_mode(conf)
    chk(f"capture: {mode}", True)
    if mode == "wayland":
        chk("gst-launch-1.0 installed", shutil.which("gst-launch-1.0"), "install gstreamer")
        chk("GStreamer pipewiresrc", subprocess.run(["gst-inspect-1.0", "pipewiresrc"], capture_output=True).returncode == 0,
            "install gst-plugin-pipewire / gstreamer1.0-pipewire")
        try:
            chk(f"screen-sharing portal v{wayland.ScreenCast()._prop('version')}", True)
        except Exception as e:  # noqa: BLE001 - any failure here means "no portal"
            chk("screen-sharing portal", False, str(e))
    print(f"config: {conf['_file']}" + ("" if os.path.exists(conf["_file"]) else " (not created yet: defaults)"))
    print(f"name on TV: {conf['NAME']}")
    if mode == "x11":
        try:
            g = capture_geometry(conf, mode)
            chk(f"monitor {g['name']} {g['width']}x{g['height']}" if g else "monitor: whole screen (xrandr found none)", True)
        except RuntimeError as e:
            chk("monitor", False, str(e))
    sup = paths.find_supplicant()
    state = paths.supplicant_state(sup)
    chk(f"patched wpa_supplicant {state}" + (f": {sup}" if sup else ""), state in ("ok", "external", "legacy"),
        "run scripts/build-supplicant" + (" (the patch changed since it was built)" if state == "outdated" else ""))
    if state == "legacy":
        print("note  patched wpa_supplicant is in the old in-tree location; run scripts/build-supplicant to move it")
    try:
        chk(f"encoder {capture.pick_encoder(conf['ENCODER'])} (ENCODER={conf['ENCODER']})", True)
    except RuntimeError as e:
        chk("H.264 encoder", False, str(e))
    try:
        chk("p2p-dev interface", p2p.p2p_dev_iface())
    except RuntimeError as e:
        chk("p2p-dev interface", False, str(e))
    try:
        rokus = p2p.lan_rokus()
        chk("Rokus on LAN", rokus, "none answered ECP :8060")
    except RuntimeError as e:   # no Wi-Fi interface to find the LAN from
        rokus = []
        chk("Rokus on LAN", False, str(e))
    if conf["TARGET"]:
        chk(f"target {conf['TARGET']!r} on LAN", any(r["name"].lower() == conf["TARGET"].lower() for r in rokus))
    if report:
        try:
            chk("selftest", selftest.run(conf, capture_mode(conf)))
        except (RuntimeError, OSError, subprocess.SubprocessError) as e:
            chk("selftest", False, str(e))
    sys.exit(0 if ok else 1)


def cmd_monitors(fmt=None):
    """X11 monitors for MONITOR= (the desktop app's monitor list). Empty on Wayland: the portal dialog picks there."""
    conf = load_conf()
    mons = capture.parse_monitors(capture.x11_monitors(conf["DISPLAY"])) if capture_mode(conf) == "x11" else []
    if fmt == "--json":
        print(json.dumps(mons))
        return
    for m in mons:
        print(f"{m['name']:<12} {m['width']}x{m['height']}+{m['x']}+{m['y']}" + ("  (primary)" if m["primary"] else ""))


def cmd_selftest(args):
    """Capture + encode a few seconds to a file and check it with ffprobe (no TV needed)."""
    seconds, out = 5, None
    it = iter(args)
    for a in it:
        if a == "--seconds":
            seconds = int(next(it, "5"))
        elif a == "--out":
            out = next(it, None)
        else:
            sys.exit("usage: rokast selftest [--seconds N] [--out FILE]")
    if not 1 <= seconds <= 600:
        sys.exit("--seconds must be 1..600")
    conf = load_conf()
    try:
        ok = selftest.run(conf, capture_mode(conf), seconds, out)
    except (RuntimeError, OSError, subprocess.SubprocessError) as e:
        sys.exit(f"selftest: FAIL {e}")
    sys.exit(0 if ok else 1)


def cmd_version(args=()):
    """version: print it.  --check: also say whether a newer release is out (update.py; daily, CHECK_UPDATES=0 = off)."""
    v = open(os.path.join(ROOT, "VERSION")).read().strip()
    if "--check" not in args:
        return print(v)
    res = update.check(v, load_conf()["CHECK_UPDATES"] != "0")
    msg = update.message(res, ROOT)
    if "--json" in args:
        return print(json.dumps({**res, "message": msg, "packaged": update.packaged(ROOT)}))
    print(v)
    print(msg or ("update check is off (CHECK_UPDATES=0)" if not res["enabled"] else
                  "up to date" if res["latest"] else "could not reach GitHub; will try again tomorrow"))


def cmd_ui(args=()):
    exe = os.path.join(ROOT, "ui", "target", "release", "rokast-ui")
    if not os.access(exe, os.X_OK):
        sys.exit("desktop UI not built: run scripts/install-ui (needs cargo and GTK 4 dev files)")
    os.execv(exe, [exe, *args])  # e.g. --tray: start hidden in the system tray


if __name__ == "__main__":
    a = sys.argv[1:] or ["help"]
    arg = a[1] if len(a) > 1 else None
    {"list": lambda: cmd_list(arg), "doctor": lambda: cmd_doctor(arg == "--report"), "start": lambda: cmd_start(arg),
     "selftest": lambda: cmd_selftest(a[1:]), "monitors": lambda: cmd_monitors(arg),
     "name": lambda: print(load_conf()["NAME"]),
     "config": lambda: print(load_conf()["_file"]),
     "version": lambda: cmd_version(a[1:]),
     "ui": lambda: cmd_ui(a[1:])}.get(
        a[0], lambda: sys.exit("usage: rokast {list [--json]|start [name]|name|config|doctor [--report]|selftest [--seconds N] [--out FILE]|"
                          "monitors [--json]|ui [--tray]|version [--check [--json]]}"))()
