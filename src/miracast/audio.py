"""Where the desktop's sound goes while mirroring (AUDIO=), through pactl (PulseAudio or PipeWire's pulse server).

  tv (= 1, default)  a null sink "rokast" becomes the default output: streams follow it, the TV gets the sound (we
                     record rokast.monitor) and the laptop speakers go quiet
  app:<name>         only that app's streams move to "rokast" (and new ones it opens, via `pactl subscribe`)
  device:<sink>      record that output's monitor; nothing is moved
  both               record the default output's monitor (the laptop keeps playing too; rokast <= 0.4)
  off (= 0)          no sound to the TV

Streams that follow the default output are not moved by hand: a moved stream can be remembered against that output
by the sound server. Only streams pinned elsewhere (tv) or the chosen app's (app:) are moved, and they go back to
where they were, or to @DEFAULT_SINK@ (which makes them follow the default again) if that was the default.
What changed is written to $XDG_RUNTIME_DIR/rokast/audio.json first, so a stop, a SIGTERM or the next start
(recover()) puts it back.
"""
import json
import os
import re
import subprocess
import threading
import time

import update

SINK = "rokast"
NEW_INPUT = re.compile(r"Event 'new' on sink-input #(\d+)")


def parse_mode(value):
    """Pure: AUDIO= value -> (kind, argument). RuntimeError names the choices."""
    v = (value or "").strip()
    low = v.lower()
    if low in ("1", "tv", ""):
        return "tv", ""
    if low in ("0", "off"):
        return "off", ""
    if low == "both":
        return "both", ""
    for kind in ("app", "device"):
        if low.startswith(kind + ":") and v[len(kind) + 1:].strip():
            return kind, v[len(kind) + 1:].strip()
    raise RuntimeError(f"AUDIO must be tv, both, off, app:<name> or device:<output> (got {value!r}; see `rokast audio`)")


def app_of(inp):
    """Pure: a sink-input's app as shown and matched: application.name, else the binary."""
    p = inp.get("properties", {})
    return p.get("application.name") or p.get("application.process.binary") or ""


def matches(inp, app):
    """Pure: does this sink-input belong to `app`? application.name or binary, case-insensitive."""
    p = inp.get("properties", {})
    want = app.lower()
    return any((p.get(k) or "").lower() == want for k in ("application.name", "application.process.binary"))


def to_move(inputs, kind, app, sink_index):
    """Pure: [(input index, its sink index)] that should move onto our sink (index `sink_index`)."""
    return [(i["index"], i["sink"]) for i in inputs
            if i["sink"] != sink_index and (kind == "tv" or (kind == "app" and matches(i, app)))]


def parse_short(text, what):
    """Pure: `pactl list short sinks|sink-inputs` -> the JSON listing's shape (no properties): pactl < 16 has no -f json."""
    out = []
    for ln in text.splitlines():
        f = ln.split("\t")
        if len(f) >= 2 and f[0].isdigit():
            out.append({"index": int(f[0]), "name": f[1], "description": f[1]} if what == "sinks"
                       else {"index": int(f[0]), "sink": int(f[1]) if f[1].isdigit() else -1, "properties": {}})
    return out


def state_file(env=os.environ):
    return os.path.join(os.path.dirname(update.lock_file(env)), "audio.json")


def pactl(*args, check=True):
    return subprocess.run(["pactl", *args], capture_output=True, text=True, timeout=10, check=check).stdout


def pactl_json(what):
    try:
        return json.loads(pactl("-f", "json", "list", what) or "[]")
    except (subprocess.CalledProcessError, ValueError):
        return parse_short(pactl("list", "short", what), what)


def default_sink():
    try:
        return pactl("get-default-sink").strip()
    except subprocess.CalledProcessError:   # pactl < 15
        return next((ln.split(":", 1)[1].strip() for ln in pactl("info").splitlines() if ln.startswith("Default Sink:")), "")


def sink_names():
    return {s["index"]: s["name"] for s in pactl_json("sinks")}


def listing():
    """{outputs: [{name, description}], apps: [{app, media}]} for `rokast audio` and the app's menu."""
    outs = [{"name": s["name"], "description": s.get("description", s["name"])} for s in pactl_json("sinks")
            if s["name"] != SINK]
    apps, seen = [], set()
    for i in pactl_json("sink-inputs"):
        a = app_of(i)
        if a and a.lower() not in seen:
            seen.add(a.lower())
            apps.append({"app": a, "media": i.get("properties", {}).get("media.name", "")})
    return {"outputs": outs, "apps": apps}


class Router:
    """route() once the TV is ready to play (returns the source for ffmpeg, or None), restore() in every exit path.
    Both are idempotent."""

    def __init__(self, value, env=os.environ):
        self.kind, self.arg = parse_mode(value)
        self.env = env
        self.state = None
        self.source = None
        self._watch = None
        self._lock = threading.Lock()

    def route(self):
        if self.source or self.kind == "off":
            return self.source
        if self.kind == "both":
            sink = default_sink()
            self.source = sink + ".monitor" if sink else None
        elif self.kind == "device":
            if self.arg not in sink_names().values():
                raise RuntimeError(f"AUDIO=device:{self.arg}: no such output (see `rokast audio`)")
            self.source = self.arg + ".monitor"
        else:
            self._take_over()
            self.source = SINK + ".monitor"
        return self.source

    def _save(self):
        os.makedirs(os.path.dirname(state_file(self.env)), exist_ok=True)
        with open(state_file(self.env), "w") as f:
            json.dump(self.state, f)

    def _ours(self):
        return next((i for i, n in sink_names().items() if n == SINK), None)

    def _take_over(self):
        old = default_sink()
        if old == SINK:   # left over and not recovered: fall back to any real output
            old = next((n for n in sink_names().values() if n != SINK), "")
        self.state = {"default": old, "module": None, "moved": {}}
        self._save()   # before changing anything: restore() must be able to undo a half-done route
        if self._ours() is None:
            self.state["module"] = pactl("load-module", "module-null-sink", f"sink_name={SINK}",
                                         'sink_properties=device.description="rokast (TV)"').strip()
            self._save()
        idx = self._ours()
        if idx is None:
            raise RuntimeError("could not create the rokast sound output (pactl load-module module-null-sink)")
        if self.kind == "tv":
            pactl("set-default-sink", SINK)
            for _ in range(10):   # streams that follow the default move by themselves; give them a moment
                if not to_move(pactl_json("sink-inputs"), "tv", "", idx):
                    break
                time.sleep(0.1)
        self._move(idx)
        if self.kind == "app":
            self._watch = subprocess.Popen(["pactl", "subscribe"], stdout=subprocess.PIPE, text=True)
            threading.Thread(target=self._follow, args=(self._watch, idx), daemon=True).start()

    def _move(self, idx):
        with self._lock:
            if self.state is None:
                return
            sinks = sink_names()
            for inp, old in to_move(pactl_json("sink-inputs"), self.kind, self.arg, idx):
                if subprocess.run(["pactl", "move-sink-input", str(inp), SINK], capture_output=True).returncode == 0:
                    self.state["moved"][str(inp)] = sinks.get(old, "")
            self._save()

    def _follow(self, proc, idx):
        """app mode: move each new stream of the app as it appears (a browser opens one per video)."""
        for line in proc.stdout:
            if NEW_INPUT.search(line):
                try:
                    self._move(idx)
                except (OSError, ValueError, subprocess.SubprocessError):
                    pass

    def restore(self):
        if self._watch:
            self._watch.terminate()
            self._watch.wait()
            self._watch = None
        with self._lock:
            if self.state:
                restore_state(self.state)
                self.state = None
                try:
                    os.remove(state_file(self.env))
                except FileNotFoundError:
                    pass
        self.source = None


def restore_target(moved_from, default, sinks):
    """Pure: where a stream we moved goes back to. Its old output if that still exists and wasn't the default,
    else @DEFAULT_SINK@ (so it follows the default again rather than being pinned to it)."""
    if moved_from and moved_from != default and moved_from != SINK and moved_from in sinks:
        return moved_from
    return "@DEFAULT_SINK@"


def restore_state(state):
    """Put back what a Router changed: default output, streams we moved, our sink unloaded."""
    sinks = sink_names()
    ours = next((i for i, n in sinks.items() if n == SINK), None)
    default = state.get("default")
    if default in sinks.values() and default != SINK:
        pactl("set-default-sink", default, check=False)
    if ours is not None:
        moved = state.get("moved", {})
        for inp in pactl_json("sink-inputs"):
            if inp["sink"] == ours:
                target = restore_target(moved.get(str(inp["index"])), default, set(sinks.values()))
                pactl("move-sink-input", str(inp["index"]), target, check=False)
    if state.get("module"):
        pactl("unload-module", str(state["module"]), check=False)


def recover(env=os.environ):
    """A previous session died without restoring: do it now. Returns True if there was something to undo."""
    try:
        with open(state_file(env)) as f:
            state = json.load(f)
    except (FileNotFoundError, ValueError):
        return False
    restore_state(state)
    os.remove(state_file(env))
    return True
