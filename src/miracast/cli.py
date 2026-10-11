"""rokast CLI: list [--json] | start [name] | doctor | ui.  Foreground; Ctrl+C stops and tears the link down."""
import asyncio
import json
import os
import signal
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capture  # noqa: E402
import p2p  # noqa: E402
import wfd  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def load_conf():
    conf = {"TARGET": "", "BITRATE": "8M", "ENCODER": "auto", "AUDIO": "1",
            "DISPLAY": ":0", "RES": "1080p30,720p60,720p30", "NAME": "Laptop"}
    path = os.path.join(ROOT, "config", "rokast.conf")
    if os.path.exists(path):
        for ln in open(path):
            ln = ln.split("#")[0].strip()
            if "=" in ln:
                k, v = ln.split("=", 1)
                conf[k.strip()] = v.strip()
    conf.update({k: os.environ[k] for k in conf if k in os.environ})
    return conf


def status(state, detail=""):
    """Machine-readable progress line (the UI keys off these): scanning, joining, waiting, streaming, stopped, error."""
    print(f"status: {state}" + (f" {detail}" if detail else ""), flush=True)


def cmd_list(fmt=None):
    rokus = p2p.lan_rokus()
    if fmt == "--json":
        print(json.dumps(rokus))
        return
    for r in rokus:
        print(f"{r['name']:<24} {r['ip']:<15} {r['model']}  p2p={r['p2p_mac']}")


async def stream(conf, iface):
    proc = {"p": None}

    async def on_play(n, ip, server_port):
        cmd = capture.build_cmd(n, ip, server_port, conf["DISPLAY"], conf["ENCODER"], conf["BITRATE"],
                                conf["AUDIO"] == "1" and n.audio == "AAC", capture.default_monitor())
        print("ffmpeg:", " ".join(cmd), flush=True)
        proc["p"] = capture.start(cmd, conf["DISPLAY"])
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
        sys.exit("no target: `rokast start \"<name>\"` or set TARGET in config/rokast.conf (see `rokast list`)")
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    dev = None
    try:
        status("scanning", name)
        target = p2p.pick(p2p.lan_rokus(), name)
        print(f"target: {target['name']} ({target['ip']}, p2p {target['p2p_mac']})", flush=True)
        conf["ENCODER"] = capture.pick_encoder(conf["ENCODER"])   # probe before joining: the TV's RTSP window is short
        print(f"encoder: {conf['ENCODER']}", flush=True)
        dev = p2p.p2p_dev_iface()
        status("joining", target["name"])
        print("joining the TV's Wi-Fi Direct group...", flush=True)
        iface = p2p.connect(dev, target, conf["NAME"])
        asyncio.run(stream(conf, iface))
    except KeyboardInterrupt:
        print("stopping", flush=True)
    except (RuntimeError, OSError, subprocess.SubprocessError) as e:
        status("error", str(e))
        sys.exit(f"error: {e}")
    finally:
        if dev:
            p2p.disconnect(dev)
    status("stopped")


def cmd_doctor():
    import shutil
    conf = load_conf()
    ok = True

    def chk(label, cond, extra=""):
        nonlocal ok
        print(("ok    " if cond else "FAIL  ") + label + (f"  ({extra})" if extra and not cond else ""))
        ok = ok and bool(cond)

    for t in ("ffmpeg", "wpa_cli", "dhcpcd", "iw"):
        chk(f"{t} installed", shutil.which(t))
    chk("patched wpa_supplicant built", os.access(p2p.SUPPLICANT, os.X_OK), "run scripts/build-supplicant")
    try:
        chk(f"encoder {capture.pick_encoder(conf['ENCODER'])} (ENCODER={conf['ENCODER']})", True)
    except RuntimeError as e:
        chk("H.264 encoder", False, str(e))
    try:
        chk("p2p-dev interface", p2p.p2p_dev_iface())
    except RuntimeError as e:
        chk("p2p-dev interface", False, str(e))
    rokus = p2p.lan_rokus()
    chk("Rokus on LAN", rokus, "none answered ECP :8060")
    if conf["TARGET"]:
        chk(f"target {conf['TARGET']!r} on LAN", any(r["name"].lower() == conf["TARGET"].lower() for r in rokus))
    sys.exit(0 if ok else 1)


def cmd_ui():
    exe = os.path.join(ROOT, "ui", "target", "release", "rokast-ui")
    if not os.access(exe, os.X_OK):
        sys.exit("desktop UI not built: run scripts/install-ui (needs cargo and GTK 4 dev files)")
    os.execv(exe, [exe])


if __name__ == "__main__":
    a = sys.argv[1:] or ["help"]
    arg = a[1] if len(a) > 1 else None
    {"list": lambda: cmd_list(arg), "doctor": cmd_doctor, "start": lambda: cmd_start(arg),
     "ui": cmd_ui}.get(
        a[0], lambda: sys.exit("usage: rokast {list [--json]|start [name]|doctor|ui}"))()
