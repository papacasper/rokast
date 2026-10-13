"""Runs cases.json through the Python implementation; prints one JSON result per line ({"ok": v} or {"err": msg})."""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "src", "miracast"))

import audio  # noqa: E402
import capture  # noqa: E402
import cli  # noqa: E402
import p2p  # noqa: E402
import selftest  # noqa: E402
import update  # noqa: E402
import wfd  # noqa: E402


def neg(d):
    return wfd.Negotiated(**d)


def build_cmd(n, sink_ip, server_port, opts):
    return capture.build_cmd(neg(n), sink_ip, server_port, **opts)


def selftest_check(probe, res, seconds, audio):
    return selftest.check(probe, selftest.negotiated(res), seconds, audio)


def negotiated(res):
    n = selftest.negotiated(res).__dict__
    return {k: v for k, v in n.items() if k != "video_param"}


FNS = {
    "parse_video_formats": wfd.parse_video_formats,
    "choose_video": lambda caps, prefs: wfd.choose_video(wfd.parse_video_formats(caps), prefs),
    "format_video_param": wfd.format_video_param,
    "parse_params": wfd.parse_params,
    "build_cmd": build_cmd,
    "probe_args": capture._probe_args,
    "parse_monitors": lambda text: capture.parse_monitors(text),
    "monitor_geometry": lambda text, name: capture.monitor_geometry(text, name),
    "clean_name": cli.clean_name,
    "parse_version": update.parse_version,
    "is_newer": update.is_newer,
    "update_message": update.message,
    "negotiated": negotiated,
    "selftest_check": selftest_check,
    "selftest_summary": selftest.summary,
    "probe_cmd": selftest.probe_cmd,
    "parse_mode": audio.parse_mode,
    "app_of": audio.app_of,
    "matches": audio.matches,
    "to_move": audio.to_move,
    "parse_short": audio.parse_short,
    "restore_target": lambda moved_from, default, sinks: audio.restore_target(moved_from, default, set(sinks)),
    "p2p_mac_of": p2p.p2p_mac_of,
    "iface_mac": p2p.iface_mac,
    "assoc_ies": p2p.assoc_ies,
    "pick": p2p.pick,
    "parse_peer": p2p.parse_peer,
}

with open(os.path.join(HERE, "cases.json")) as f:
    cases = json.load(f)
for case in cases:
    try:
        out = {"ok": FNS[case["fn"]](**case["args"])}
    except TypeError as e:
        if "argument" in str(e):  # a case that doesn't fit the function: the harness is wrong, not the code
            sys.exit(f"parity: case {case}: {e}")
        out = {"err": str(e)}
    except Exception as e:  # noqa: BLE001 - every failure is a result to compare
        out = {"err": str(e)}
    print(json.dumps(out, sort_keys=True, ensure_ascii=False))
