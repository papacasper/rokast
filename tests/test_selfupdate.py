"""selfupdate.py pieces that guard the download: allowed hosts and safe tarball extraction.
The whole `rokast update` flow is tests/test_self_update.sh."""
import io
import os
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "src", "miracast"))
import selfupdate  # noqa: E402


def make_tar(path, members):
    """members: (name, kind, data/linkname) with kind 'file', 'dir', 'sym', 'hard' or 'fifo'."""
    with tarfile.open(path, "w:gz") as t:
        for name, kind, extra in members:
            ti = tarfile.TarInfo(name)
            ti.type = {"file": tarfile.REGTYPE, "dir": tarfile.DIRTYPE, "sym": tarfile.SYMTYPE,
                       "hard": tarfile.LNKTYPE, "fifo": tarfile.FIFOTYPE}[kind]
            if kind in ("sym", "hard"):
                ti.linkname = extra
            data = extra.encode() if kind == "file" else b""
            ti.size = len(data)
            t.addfile(ti, io.BytesIO(data) if data else None)


class SelfUpdateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_allowed_hosts(self):
        env = {}
        self.assertTrue(selfupdate.allowed("https://github.com/papacasper/rokast/releases/download/v1/x", env))
        self.assertTrue(selfupdate.allowed("https://objects.githubusercontent.com/x", env))
        self.assertFalse(selfupdate.allowed("http://github.com/x", env))           # not HTTPS
        self.assertFalse(selfupdate.allowed("https://github.com.evil.example/x", env))
        self.assertFalse(selfupdate.allowed("http://127.0.0.1:8000/x", env))
        env = {"ROKAST_UPDATE_URL": "http://127.0.0.1:8000/latest.json"}            # the test server's origin only
        self.assertTrue(selfupdate.allowed("http://127.0.0.1:8000/x", env))
        self.assertFalse(selfupdate.allowed("http://127.0.0.1:8001/x", env))

    def extract(self, members):
        tb = os.path.join(self.d, "r.tar.gz")
        make_tar(tb, members)
        return selfupdate.extract(tb, os.path.join(self.d, "x"), "1.0.0")

    def test_good_tarball(self):
        out = self.extract([("rokast-1.0.0", "dir", None), ("rokast-1.0.0/VERSION", "file", "1.0.0\n"),
                            ("rokast-1.0.0/link", "sym", "VERSION")])
        with open(os.path.join(out, "VERSION")) as f:
            self.assertEqual(f.read(), "1.0.0\n")

    def test_unsafe_tarballs_are_refused(self):
        for bad in ([("rokast-1.0.0/../evil", "file", "x")],
                    [("/etc/evil", "file", "x")],
                    [("other-1.0.0/VERSION", "file", "x")],
                    [("rokast-1.0.0/l", "sym", "../../outside")],
                    [("rokast-1.0.0/l", "sym", "/etc/passwd")],
                    [("rokast-1.0.0/h", "hard", "rokast-1.0.0/VERSION")],
                    [("rokast-1.0.0/p", "fifo", None)]):
            with self.subTest(bad=bad[0][0]), self.assertRaises(selfupdate.UpdateError):
                self.extract(bad)
        self.assertFalse(os.path.exists(os.path.join(self.d, "evil")))

    def test_auto_update_decision(self):
        on = {"AUTO_UPDATE": "1"}
        self.assertIsNone(selfupdate.auto_reason(on, "user", False))
        self.assertEqual(selfupdate.auto_reason({}, "user", False), "AUTO_UPDATE is off")       # default off
        self.assertEqual(selfupdate.auto_reason({"AUTO_UPDATE": "0"}, "user", False), "AUTO_UPDATE is off")
        self.assertEqual(selfupdate.auto_reason({**on, "CHECK_UPDATES": "0"}, "user", False), "CHECK_UPDATES=0")
        self.assertEqual(selfupdate.auto_reason(on, "user", True), "mirroring")
        for kind in ("deb", "rpm", "arch", "git", "other"):                                 # never for packages
            self.assertEqual(selfupdate.auto_reason(on, kind, False), f"{kind} install")
            self.assertEqual(selfupdate.auto_reason(on, kind, True), f"{kind} install")

    def test_no_ssh_keygen_refuses_before_downloading(self):
        rel = {"version": "9.9.9", "assets": {}}
        env = {"PATH": self.d, "XDG_CACHE_HOME": self.d, "XDG_RUNTIME_DIR": self.d}
        out = io.StringIO()
        with mock.patch.object(selfupdate, "latest_release", return_value=rel), \
                mock.patch.object(selfupdate.update, "kind", return_value="user"), \
                mock.patch.object(selfupdate, "download") as dl, mock.patch("sys.stdout", out):
            self.assertEqual(selfupdate.run_update(self.d, "0.1.0", yes=True, env=env), 1)
        dl.assert_not_called()
        self.assertIn("update: error checking the release signature needs ssh-keygen", out.getvalue())


if __name__ == "__main__":
    unittest.main()
