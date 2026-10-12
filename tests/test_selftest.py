"""rokast selftest checks + hardware report helpers (src/miracast/selftest.py)."""
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "src", "miracast"))
import selftest  # noqa: E402


def probe(frames=150, w=1920, h=1080, codec="h264", audio=True):
    s = [{"codec_type": "video", "codec_name": codec, "width": w, "height": h, "nb_read_frames": str(frames)}]
    if audio:
        s.append({"codec_type": "audio", "codec_name": "aac"})
    return {"streams": s}


class Check(unittest.TestCase):
    n = selftest.negotiated("1080p30")

    def test_negotiated_takes_first_known_mode(self):
        n = selftest.negotiated("4k, 720p60,1080p30")
        self.assertEqual((n.width, n.height, n.fps, n.profile), (1280, 720, 60, "high"))
        self.assertEqual(selftest.negotiated("1080p60").level, "4.2")
        with self.assertRaises(RuntimeError):
            selftest.negotiated("4k")

    def test_good_file_passes(self):
        self.assertEqual(selftest.check(probe(), self.n, 5, True), [])
        self.assertEqual(selftest.check(probe(120), self.n, 5, True), [])   # 80% of 150

    def test_each_problem_is_reported(self):
        self.assertEqual(selftest.check({"streams": []}, self.n, 5, False), ["no video stream in the output"])
        self.assertIn("frames", selftest.check(probe(119), self.n, 5, True)[0])
        self.assertIn("1280x720", selftest.check(probe(w=1280, h=720), self.n, 5, True)[0])
        self.assertIn("hevc", selftest.check(probe(codec="hevc"), self.n, 5, True)[0])
        self.assertIn("audio", selftest.check(probe(audio=False), self.n, 5, True)[0])
        self.assertEqual(selftest.check(probe(audio=False), self.n, 5, False), [])

    def test_summary(self):
        self.assertEqual(selftest.summary(probe()), "h264 1920x1080 150 frames, aac audio")


class Report(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_os_release(self):
        p = os.path.join(self.tmp, "os-release")
        with open(p, "w") as f:
            f.write('NAME="X"\nPRETTY_NAME="Manjaro Linux"\n')
        self.assertEqual(selftest.os_release(p), "Manjaro Linux")
        self.assertEqual(selftest.os_release(os.path.join(self.tmp, "nope")), "unknown")

    def test_wifi_chips_finds_wireless_ifaces_and_driver(self):
        drv = os.path.join(self.tmp, "drivers", "iwlwifi")
        os.makedirs(drv)
        os.makedirs(os.path.join(self.tmp, "net", "wlo1", "wireless"))
        os.makedirs(os.path.join(self.tmp, "net", "wlo1", "device"))
        os.symlink(drv, os.path.join(self.tmp, "net", "wlo1", "device", "driver"))
        os.makedirs(os.path.join(self.tmp, "net", "eth0"))
        self.assertEqual(selftest.wifi_chips(os.path.join(self.tmp, "net")), [("wlo1", "iwlwifi")])


if __name__ == "__main__":
    unittest.main()
