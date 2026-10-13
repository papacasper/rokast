"""Sound routing (src/miracast/audio.py): pure parts, and Router/recover against tests/fake_pactl.py."""
import json
import os
import sys
import tempfile
import time
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "src", "miracast"))
import audio  # noqa: E402

SPEAKERS, HDMI = "alsa_output.speakers", "alsa_output.hdmi"


class Pure(unittest.TestCase):
    def test_parse_mode(self):
        for v, want in [("1", ("tv", "")), ("", ("tv", "")), ("TV", ("tv", "")), ("0", ("off", "")),
                        ("off", ("off", "")), ("both", ("both", "")), ("app:Firefox", ("app", "Firefox")),
                        ("device: alsa_output.hdmi ", ("device", "alsa_output.hdmi"))]:
            self.assertEqual(audio.parse_mode(v), want, v)
        for bad in ("yes", "app:", "device:", "2"):
            with self.assertRaisesRegex(RuntimeError, "AUDIO must be"):
                audio.parse_mode(bad)

    def test_app_matching(self):
        inp = {"index": 5, "sink": 1, "properties": {"application.name": "Firefox", "application.process.binary": "firefox-bin"}}
        self.assertEqual(audio.app_of(inp), "Firefox")
        self.assertTrue(audio.matches(inp, "firefox"))
        self.assertTrue(audio.matches(inp, "FIREFOX-BIN"))
        self.assertFalse(audio.matches(inp, "fire"))
        self.assertEqual(audio.app_of({"properties": {"application.process.binary": "mpv"}}), "mpv")

    def test_to_move(self):
        ins = [{"index": 1, "sink": 7, "properties": {"application.name": "mpv"}},
               {"index": 2, "sink": 9, "properties": {"application.name": "Firefox"}},
               {"index": 3, "sink": 7, "properties": {"application.name": "Firefox"}}]
        self.assertEqual(audio.to_move(ins, "tv", "", 9), [(1, 7), (3, 7)])
        self.assertEqual(audio.to_move(ins, "app", "firefox", 9), [(3, 7)])
        self.assertEqual(audio.to_move(ins, "both", "", 9), [])

    def test_restore_target(self):
        sinks = {SPEAKERS, HDMI, "rokast"}
        self.assertEqual(audio.restore_target(HDMI, SPEAKERS, sinks), HDMI)
        self.assertEqual(audio.restore_target(SPEAKERS, SPEAKERS, sinks), "@DEFAULT_SINK@")   # don't pin it
        self.assertEqual(audio.restore_target("gone", SPEAKERS, sinks), "@DEFAULT_SINK@")
        self.assertEqual(audio.restore_target(None, SPEAKERS, sinks), "@DEFAULT_SINK@")

    def test_parse_short(self):
        self.assertEqual(audio.parse_short("61\talsa_output.x\tPipeWire\ts32le 2ch 48000Hz\tIDLE\n", "sinks"),
                         [{"index": 61, "name": "alsa_output.x", "description": "alsa_output.x"}])
        self.assertEqual(audio.parse_short("10148\t61\t10147\tPipeWire\ts16le 2ch 48000Hz\n\n", "sink-inputs"),
                         [{"index": 10148, "sink": 61, "properties": {}}])

    def test_recorded_pipewire_listing(self):
        """Shape of a real `pactl -f json list sink-inputs` entry (PipeWire 1.6.8, trimmed)."""
        inp = {"index": 10130, "driver": "PipeWire", "client": "10129", "sink": 61, "corked": False,
               "properties": {"application.name": "rokast-test-a", "application.process.binary": "pacat",
                              "media.name": "tone.wav", "node.name": "rokast-test-a"}}
        self.assertEqual(audio.to_move([inp], "app", "rokast-test-a", 99), [(10130, 61)])
        self.assertEqual(audio.app_of(inp), "rokast-test-a")


class Routing(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp()
        bin_ = os.path.join(self.t, "bin")
        os.mkdir(bin_)
        os.symlink(os.path.join(REPO, "tests", "fake_pactl.py"), os.path.join(bin_, "pactl"))
        self.state = os.path.join(self.t, "pactl.json")
        self.write({"default": SPEAKERS, "sinks": {"1": SPEAKERS, "2": HDMI}, "modules": {}, "next": 10,
                    "inputs": {"100": {"sink": 1, "app": "mpv", "pinned": False},
                               "101": {"sink": 2, "app": "Firefox", "pinned": True},
                               "102": {"sink": 1, "app": "Firefox", "pinned": False}}})
        self.old_env = dict(os.environ)
        os.environ.update(PATH=bin_ + os.pathsep + os.environ["PATH"], FAKE_PACTL_STATE=self.state,
                          FAKE_PACTL_LOG=os.path.join(self.t, "log"), XDG_RUNTIME_DIR=self.t)
        self.before = self.read()

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.old_env)

    def write(self, s):
        with open(self.state, "w") as f:
            json.dump(s, f)

    def read(self):
        with open(self.state) as f:
            return json.load(f)

    def saved(self):
        with open(audio.state_file()) as f:
            return json.load(f)

    def sink_of(self, s, inp):
        return s["sinks"][str(s["inputs"][inp]["sink"])]

    def assert_restored(self):
        s = self.read()
        self.assertEqual(s["default"], SPEAKERS)
        self.assertEqual(s["sinks"], self.before["sinks"])
        self.assertEqual(s["modules"], {})
        for i, v in self.before["inputs"].items():
            self.assertEqual((s["inputs"][i]["sink"], s["inputs"][i]["pinned"]), (v["sink"], v["pinned"]), i)
        self.assertFalse(os.path.exists(audio.state_file()))

    def test_tv_only(self):
        r = audio.Router("1")
        self.assertEqual(r.route(), "rokast.monitor")
        self.assertEqual(r.route(), "rokast.monitor")   # idempotent
        s = self.read()
        self.assertEqual(s["default"], "rokast")
        self.assertTrue(all(self.sink_of(s, i) == "rokast" for i in s["inputs"]))
        self.assertEqual(self.saved()["moved"], {"101": HDMI})   # only the pinned one
        r.restore()
        r.restore()
        self.assert_restored()

    def test_app_only_and_new_streams(self):
        r = audio.Router("app:firefox")
        r.route()
        s = self.read()
        self.assertEqual(s["default"], SPEAKERS)
        self.assertEqual(self.sink_of(s, "100"), SPEAKERS)
        self.assertEqual({self.sink_of(s, "101"), self.sink_of(s, "102")}, {"rokast"})
        time.sleep(0.3)   # let the watcher start
        s["inputs"]["103"] = {"sink": 1, "app": "Firefox", "pinned": False}
        s["inputs"]["104"] = {"sink": 1, "app": "mpv", "pinned": False}
        self.write(s)
        deadline = time.time() + 1
        while time.time() < deadline and self.sink_of(self.read(), "103") != "rokast":
            time.sleep(0.05)
        self.assertEqual(self.sink_of(self.read(), "103"), "rokast")
        self.assertEqual(self.sink_of(self.read(), "104"), SPEAKERS)
        r.restore()
        s = self.read()
        self.assertEqual((s["inputs"]["103"]["sink"], s["inputs"]["103"]["pinned"]), (1, False))
        del s["inputs"]["103"], s["inputs"]["104"]
        self.write(s)
        self.assert_restored()

    def test_crash_then_recover(self):
        audio.Router("tv").route()   # never restored
        self.assertEqual(self.read()["default"], "rokast")
        self.assertTrue(audio.recover())
        self.assert_restored()
        self.assertFalse(audio.recover())

    def test_device_both_off(self):
        self.assertEqual(audio.Router(f"device:{HDMI}").route(), HDMI + ".monitor")
        with self.assertRaisesRegex(RuntimeError, "no such output"):
            audio.Router("device:nope").route()
        self.assertEqual(audio.Router("both").route(), SPEAKERS + ".monitor")
        self.assertIsNone(audio.Router("0").route())
        self.assertEqual(self.read(), self.before)

    def test_old_pactl(self):
        os.environ["FAKE_PACTL_OLD"] = "1"
        r = audio.Router("tv")
        self.assertEqual(r.route(), "rokast.monitor")
        self.assertEqual(self.read()["default"], "rokast")
        r.restore()
        self.assert_restored()

    def test_leftover_default(self):
        """A dead session left 'rokast' as the default and its state file got lost: still restore a real output."""
        r = audio.Router("tv")
        r.route()
        os.remove(audio.state_file())
        r.state = None
        r2 = audio.Router("tv")
        r2.route()
        self.assertEqual(self.saved()["default"], SPEAKERS)
        r2.restore()
        self.assertEqual(self.read()["default"], SPEAKERS)

    def test_listing(self):
        got = audio.listing()
        self.assertEqual([o["name"] for o in got["outputs"]], [SPEAKERS, HDMI])
        self.assertEqual([a["app"] for a in got["apps"]], ["mpv", "Firefox"])


if __name__ == "__main__":
    unittest.main()
