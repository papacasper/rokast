"""Writes cases.json: the inputs both py_side.py and the Rust parity example run through the pure functions.

Regenerate after adding cases: python3 -I tests/parity/make_cases.py. Each case is {fn, args}; "exact_err": true
means an error's message must match too (it is shown to people), otherwise only "both failed" is compared.
"""
import itertools
import json
import os

ROKU = "00 00 03 10 0001ffff 1fffffff 00000fff 00 0000 0000 11 none none"
XRANDR = ("Monitors: 2\n 0: +*eDP-1 1920/344x1080/194+0+0  eDP-1\n"
          " 1: +HDMI-1-0 2560/597x1440/336+1920+0  HDMI-1-0\n")
N1080 = {"width": 1920, "height": 1080, "fps": 30, "profile": "high", "level": "4.0", "audio": "AAC", "rtp_port": 1028}
N720 = {"width": 1280, "height": 720, "fps": 60, "profile": "baseline", "level": "4.2", "audio": "LPCM",
        "rtp_port": 16384}
GEO = {"name": "HDMI-1-0", "width": 2560, "height": 1440, "x": 1920, "y": 0, "primary": False}


def c(fn, exact_err=False, **args):
    return {"fn": fn, "args": args, **({"exact_err": True} if exact_err else {})}


def probe(frames, w, h, codec, audio):
    s = [{"codec_type": "video", "codec_name": codec, "width": w, "height": h, "nb_read_frames": str(frames)}]
    return {"streams": s + ([{"codec_type": "audio", "codec_name": "aac"}] if audio else [])}


cases = []
# wfd
for s in [ROKU, "00 00 01 02 00000080 00000000 00000000 00 0000 0000 00 none none", "none", "00 00 zz", ""]:
    cases.append(c("parse_video_formats", True, s=s))
for caps, prefs in itertools.product(
        [ROKU, "00 00 01 02 00000080 00000000 00000000 00 0000 0000 00 none none",
         "00 00 02 00 00000001 00000000 00000000 00 0000 0000 00 none none",
         "00 00 01 04 00000004 00000000 00000000 00 0000 0000 00 none none"],
        [[], ["720p60"], ["1080p60"], ["4k"], ["480p60", "1080p30"]]):
    cases.append(c("choose_video", True, caps=caps, prefs=prefs))
for bit, prof, level in [(7, 1, 4), (8, 2, 0x10), (0, 1, 1)]:
    cases.append(c("format_video_param", bit=bit, prof=prof, level=level))
cases.append(c("parse_params", body="wfd_video_formats: 00 00\r\nwfd_audio_codecs: AAC 00000001 00\r\nnot a param\n"))
# capture
for enc, src, audio, out, geo in itertools.product(
        ["h264_nvenc", "h264_vaapi", "libx264", "libopenh264"], ["x11", "raw"], [True, False],
        [None, "/tmp/x.ts"], [None, GEO]):
    if src == "raw" and geo:
        continue
    for n in [N1080, N720]:
        opts = {"display": ":0.0", "encoder": enc, "bitrate": "6M", "audio": audio,
                "monitor": "rokast.monitor" if audio else None, "source": src, "out": out,
                "seconds": 5 if out else None, "geometry": geo}
        cases.append(c("build_cmd", n=n, sink_ip=None if out else "192.168.49.1", server_port=None if out else 5000,
                       opts=opts))
for enc in ["h264_nvenc", "h264_vaapi", "libx264", "libopenh264"]:
    cases.append(c("probe_args", encoder=enc))
for text in [XRANDR, "", "Monitors: 1\n 0: +DP-1 3840/600x2160/340+0+0  DP-1\n", "garbage\n 0: nope\n"]:
    cases.append(c("parse_monitors", text=text))
for text, name in [(XRANDR, ""), (XRANDR, "HDMI-1-0"), (XRANDR, "DP-9"), ("", "x"),
                   ("Monitors: 1\n 0: +DP-1 3840/600x2160/340+0+0  DP-1\n", "")]:
    cases.append(c("monitor_geometry", True, text=text, name=name))
# conf
for name in ["  Casper's Laptop\n", "é" * 40, "\x00\x07", "Den PC", "a" * 31 + "é", "tab\there"]:
    cases.append(c("clean_name", name=name))
# update
for v in ["v0.10.2", "0.4.0", "0.4.0-rc1", "", "garbage", "v1"]:
    cases.append(c("parse_version", v=v))
for a, b in [("v0.5.0", "0.4.0"), ("0.4.0", "0.4.0"), ("0.10.0", "0.9.9"), ("x", "0.1.0"), ("0.1.0", ""),
             ("1.0", "0.9.9")]:
    cases.append(c("is_newer", latest=a, current=b))
for newer, root in itertools.product([True, False], ["/usr/share/rokast", "/home/u/.local/share/rokast"]):
    res = {"current": "0.4.0", "latest": "0.5.0", "newer": newer, "checked": True, "enabled": True,
           "url": "https://github.com/papacasper/rokast/releases/latest"}
    cases.append(c("update_message", res=res, root=root))
# selftest
for res in ["1080p30", "4k, 720p60,1080p30", "1080p60", "4k", ""]:
    cases.append(c("negotiated", True, res=res))
for p, seconds, audio in [(probe(150, 1920, 1080, "h264", True), 5, True), (probe(119, 1920, 1080, "h264", True), 5, True),
                          (probe(150, 1280, 720, "hevc", False), 5, True), ({"streams": []}, 5, False),
                          (probe(150, 1920, 1080, "h264", False), 5, False)]:
    cases.append(c("selftest_check", probe=p, res="1080p30", seconds=seconds, audio=audio))
    cases.append(c("selftest_summary", probe=p))
cases.append(c("probe_cmd", path="/tmp/out.ts"))
# audio
for v in ["1", "", "TV", "0", "off", "both", "app:Firefox", "device: alsa_output.hdmi ", "yes", "app:", "2"]:
    cases.append(c("parse_mode", True, value=v))
INPUTS = [{"index": 1, "sink": 7, "properties": {"application.name": "mpv"}},
          {"index": 2, "sink": 9, "properties": {"application.name": "Firefox"}},
          {"index": 3, "sink": 7, "properties": {"application.process.binary": "firefox"}},
          {"index": 4, "sink": 7, "properties": {}}]
for inp in INPUTS:
    cases.append(c("app_of", inp=inp))
    for app in ["firefox", "MPV", "fire"]:
        cases.append(c("matches", inp=inp, app=app))
for kind, app in [("tv", ""), ("app", "firefox"), ("both", ""), ("device", "x")]:
    cases.append(c("to_move", inputs=INPUTS, kind=kind, app=app, sink_index=9))
for text, what in [("61\talsa_output.x\tPipeWire\ts32le 2ch 48000Hz\tIDLE\n", "sinks"),
                   ("10148\t61\t10147\tPipeWire\ts16le 2ch 48000Hz\n\nx\ty\n", "sink-inputs"), ("", "sinks")]:
    cases.append(c("parse_short", text=text, what=what))
for frm in ["alsa_output.hdmi", "alsa_output.speakers", "gone", None, "rokast", ""]:
    cases.append(c("restore_target", moved_from=frm, default="alsa_output.speakers",
                   sinks=["alsa_output.speakers", "alsa_output.hdmi", "rokast"]))
# p2p
for mac in ["4c:50:dd:21:8b:c4", "a8:16:9d:d4:b8:c5", "00:11:22:33:c4:55"]:
    cases.append(c("p2p_mac_of", wifi_mac=mac))
    cases.append(c("iface_mac", dev_addr=mac))
    cases.append(c("assoc_ies", dev_addr=mac, name="Casper's ThinkPad"))
cases.append(c("assoc_ies", dev_addr="02:11:22:33:44:55", name="x" * 40))
ROKUS = [{"ip": "192.168.0.63", "name": "Living Room", "model": "", "wifi_mac": "", "p2p_mac": ""},
         {"ip": "192.168.0.64", "name": "Den", "model": "", "wifi_mac": "", "p2p_mac": ""},
         {"ip": "192.168.0.65", "name": "Den", "model": "", "wifi_mac": "", "p2p_mac": ""}]
for name in ["living room", "Den", "JERICAS TV"]:
    cases.append(c("pick", True, rokus=ROKUS, name=name))
cases.append(c("parse_peer", text="4e:11:22:33:44:55\ndevice_name=Living Room\nwfd=a=b\n"))

with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "cases.json"), "w") as f:
    json.dump(cases, f, indent=0, ensure_ascii=False)
    f.write("\n")
print(f"{len(cases)} cases")
