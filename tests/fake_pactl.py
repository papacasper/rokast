#!/usr/bin/env python3
"""A stand-in `pactl` for tests/test_audio.py: a sound server's routing in a JSON file ($FAKE_PACTL_STATE).

Behaves like PipeWire/PulseAudio >= 15: streams follow the default output unless pinned; a move to a named output
pins the stream, a move to @DEFAULT_SINK@ unpins it; unloading a sink sends its streams to the default.
FAKE_PACTL_OLD=1 acts like pactl < 15: no -f json, no get-default-sink. Every call is appended to $FAKE_PACTL_LOG.
"""
import json
import os
import sys
import time

P = os.environ["FAKE_PACTL_STATE"]
OLD = os.environ.get("FAKE_PACTL_OLD") == "1"


def load():
    with open(P) as f:
        return json.load(f)


def save(s):
    with open(P + ".tmp", "w") as f:
        json.dump(s, f)
    os.replace(P + ".tmp", P)


def by_name(s, name):
    return next((int(i) for i, n in s["sinks"].items() if n == name), None)


def main(a):
    with open(os.environ.get("FAKE_PACTL_LOG", os.devnull), "a") as log:
        log.write(" ".join(a) + "\n")
    s = load()
    if a[:2] == ["-f", "json"]:
        if OLD:
            sys.exit("pactl: invalid option -- 'f'")
        what = a[3]
        if what == "sinks":
            out = [{"index": int(i), "name": n, "description": n.title()} for i, n in s["sinks"].items()]
        else:
            out = [{"index": int(i), "sink": v["sink"], "properties": {"application.name": v["app"],
                    "application.process.binary": v.get("bin", "pacat"), "media.name": v["app"] + " media"}}
                   for i, v in s["inputs"].items()]
        print(json.dumps(out))
    elif a[:2] == ["list", "short"]:
        if a[2] == "sinks":
            print("\n".join(f"{i}\t{n}\tPipeWire\ts16le 2ch 48000Hz\tIDLE" for i, n in s["sinks"].items()))
        else:
            print("\n".join(f"{i}\t{v['sink']}\t99\tPipeWire\ts16le 2ch 48000Hz" for i, v in s["inputs"].items()))
    elif a == ["get-default-sink"]:
        if OLD:
            sys.exit("No valid command specified.")
        print(s["default"])
    elif a == ["info"]:
        print(f"Server Name: fake\nDefault Sink: {s['default']}\nDefault Source: x")
    elif a[0] == "set-default-sink":
        idx = by_name(s, a[1])
        if idx is None:
            sys.exit("Failure: No such entity")
        s["default"] = a[1]
        for v in s["inputs"].values():
            if not v["pinned"]:
                v["sink"] = idx
        save(s)
    elif a[0] == "move-sink-input":
        if a[1] not in s["inputs"]:
            sys.exit("Failure: No such entity")
        target = s["default"] if a[2] == "@DEFAULT_SINK@" else a[2]
        idx = by_name(s, target)
        if idx is None:
            sys.exit("Failure: No such entity")
        s["inputs"][a[1]].update(sink=idx, pinned=a[2] != "@DEFAULT_SINK@")
        save(s)
    elif a[0] == "load-module":
        name = next(x.split("=", 1)[1] for x in a if x.startswith("sink_name="))
        s["next"] += 1
        s["sinks"][str(s["next"])] = name
        s["modules"][str(s["next"] + 1000)] = s["next"]
        save(s)
        print(s["next"] + 1000)
    elif a[0] == "unload-module":
        sink = s["modules"].pop(a[1], None)
        if sink is None:
            sys.exit("Failure: No such entity")
        name = s["sinks"].pop(str(sink))
        if s["default"] == name:
            s["default"] = next(iter(s["sinks"].values()))
        for v in s["inputs"].values():
            if v["sink"] == sink:
                v.update(sink=by_name(s, s["default"]), pinned=False)
        save(s)
    elif a == ["subscribe"]:
        seen = set(s["inputs"])
        while True:
            time.sleep(0.05)
            now = set(load()["inputs"])
            for i in sorted(now - seen, key=int):
                print(f"Event 'new' on sink-input #{i}", flush=True)
            seen = now
    else:
        sys.exit(f"fake pactl: unsupported {a}")


if __name__ == "__main__":
    main(sys.argv[1:])
