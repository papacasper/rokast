"""Join-failure evidence (src/miracast/joinlog.py + p2p.connect): what is kept, where, and how many."""
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "src", "miracast"))
import joinlog  # noqa: E402
import p2p  # noqa: E402

TARGET = {"name": "Living Room", "ip": "192.0.2.9", "model": "R", "p2p_mac": "aa:bb:cc:dd:ee:ff"}


class FakeRadio:
    """Stands in for both supplicants: each attempt 'joins' only if its number is in `joins_on`."""
    def __init__(self, joins_on=(), peer_error=None):
        self.joins_on, self.peer_error, self.attempt, self.own_calls = set(joins_on), peer_error, 0, []

    def patches(self):
        return mock.patch.multiple(
            p2p, group_iface=lambda: "p2p-wlo1-mir", disconnect=lambda *a, **k: None, _set_wfd=lambda *a: None,
            find_peer=self.find_peer, _start_own=lambda *a: None, tv_state=lambda ip: {"power_mode": "DisplayOff"},
            wpa=self.wpa, own=self.own, sh=lambda *a, **k: f"supplicant log {self.attempt}", _wait_state=self.wait)

    def find_peer(self, dev, target):
        self.attempt += 1
        if self.peer_error:
            raise RuntimeError(self.peer_error)
        return {"interface_addr": "aa:bb:cc:dd:ee:fe", "listen_freq": "2437", "oper_ssid": "DIRECT-roku-XGF-A0FA2A"}

    def wpa(self, dev, *args, check=True):
        return "p2p_device_address=11:22:33:44:55:66\nwpa_state=DISCONNECTED" if args[0] == "status" else "age=3"

    def own(self, iface, *args, check=True):
        self.own_calls.append(args)
        if args[0] == "list_networks":
            return "network id / ssid / bssid / flags\n0\tany\taa:bb:cc:dd:ee:fe\t[CURRENT]"
        return "wpa_state=SCANNING" if args[0] == "status" else "OK"

    def wait(self, iface, want, timeout):
        return self.attempt in self.joins_on


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="rokast-test-")
        self.addCleanup(shutil.rmtree, self.tmp)
        env = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": self.tmp})
        env.start()
        self.addCleanup(env.stop)

    def connect(self, radio):
        out = io.StringIO()
        with radio.patches(), contextlib.redirect_stdout(out):
            try:
                return p2p.connect("p2p-dev-wlo1", TARGET), out.getvalue(), None
            except RuntimeError as e:
                return None, out.getvalue(), str(e)

    def summary(self, d):
        with open(os.path.join(d, "summary.json")) as f:
            return json.load(f)


class Connect(Base):
    def test_all_attempts_fail_keeps_every_attempt(self):
        iface, out, err = self.connect(FakeRadio())
        self.assertIsNone(iface)
        d = joinlog.latest()
        self.assertIn(f"(details: {d})", err)
        s = self.summary(d)
        self.assertEqual(s["result"], "failed")
        self.assertEqual([a["attempt"] for a in s["attempts"]], [1, 2, 3, 4])
        self.assertEqual(s["attempts"][0]["own_status"], {"wpa_state": "SCANNING"})
        self.assertEqual(s["attempts"][0]["peer"]["listen_freq"], "2437")
        self.assertEqual(s["tv"], {"before": {"power_mode": "DisplayOff"}, "after": {"power_mode": "DisplayOff"}})
        self.assertEqual(s["target"]["name"], "Living Room")
        for n in range(1, 5):   # each attempt's own log, not the last one four times
            with open(os.path.join(d, f"attempt-{n}.log")) as f:
                self.assertEqual(f.read(), f"supplicant log {n}")

    def test_join_after_a_retry_is_kept_too(self):
        iface, out, err = self.connect(FakeRadio(joins_on={2}))
        self.assertEqual(iface, "p2p-wlo1-mir")
        d = joinlog.latest()
        self.assertIn(f"p2p: joined on attempt 2/4 (failed attempts: {d})", out)
        s = self.summary(d)
        self.assertEqual(s["result"], "joined on attempt 2")
        self.assertEqual(["failed_at" in a for a in s["attempts"]], [True, False])   # the join is kept to compare
        self.assertEqual(sorted(os.listdir(d)), ["attempt-1.log", "attempt-2.log", "summary.json"])

    def test_first_try_join_keeps_nothing(self):
        iface, out, err = self.connect(FakeRadio(joins_on={1}))
        self.assertEqual(iface, "p2p-wlo1-mir")
        self.assertIn("p2p: joined on attempt 1/4\n", out)
        self.assertIsNone(joinlog.latest())

    def test_tv_not_visible_is_kept_with_its_error(self):
        iface, out, err = self.connect(FakeRadio(peer_error="Living Room not visible over Wi-Fi Direct in 30s"))
        d = joinlog.latest()
        self.assertEqual(err, f"Living Room not visible over Wi-Fi Direct in 30s (details: {d})")
        self.assertEqual(self.summary(d)["result"], "error on attempt 1: Living Room not visible over Wi-Fi Direct in 30s")


    def test_probes_for_the_tv_by_name(self):
        radio = FakeRadio(joins_on={1})
        self.connect(radio)
        self.assertIn(("set_network", "0", "ssid", "DIRECT-roku-XGF-A0FA2A".encode().hex()), radio.own_calls)
        self.assertIn(("set_network", "0", "scan_ssid", "1"), radio.own_calls)
        i = radio.own_calls.index
        self.assertLess(i(("wps_pbc", "aa:bb:cc:dd:ee:fe")), i(("set_network", "0", "scan_ssid", "1")))


class Keep(Base):
    def test_prunes_to_newest_five(self):
        for i in range(7):
            log = joinlog.JoinLog(TARGET)
            log.failed({"start": 0}, "x")
            log.save("failed", now=1_700_000_000 + i * 60)
        kept = [os.path.basename(r) for r in joinlog.runs()]
        self.assertEqual(len(kept), 5)
        self.assertEqual(kept[-1], "20231114-221920")   # the newest (i=6) survives
        self.assertEqual(kept[0], "20231114-221520")    # i=2; i=0 and 1 pruned

    def test_same_second_does_not_overwrite(self):
        for _ in range(2):
            log = joinlog.JoinLog(TARGET)
            log.failed({}, "x")
            log.save("failed", now=1_700_000_000)
        self.assertEqual([os.path.basename(r) for r in joinlog.runs()], ["20231114-221320", "20231114-221320-2"])

    def test_unwritable_cache_does_not_break_the_join(self):
        blocker = os.path.join(self.tmp, "rokast")
        open(blocker, "w").close()   # a file where the folder should be
        log = joinlog.JoinLog(TARGET)
        log.failed({}, "x")
        self.assertIsNone(log.save("failed"))


if __name__ == "__main__":
    unittest.main()
