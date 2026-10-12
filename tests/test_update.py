"""Update notice (src/miracast/update.py + `rokast version --check`) against a local server that records every hit."""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "src", "miracast"))
import update  # noqa: E402


class FakeGitHub(http.server.BaseHTTPRequestHandler):
    hits = []
    tag = "v9.9.9"

    def do_GET(self):
        FakeGitHub.hits.append(self.path)
        body = json.dumps({"tag_name": FakeGitHub.tag, "html_url": "https://example.invalid/rel"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


class UpdateTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = http.server.HTTPServer(("127.0.0.1", 0), FakeGitHub)
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()
        cls.url = f"http://127.0.0.1:{cls.srv.server_port}/latest"

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()

    def setUp(self):
        FakeGitHub.hits.clear()
        FakeGitHub.tag = "v9.9.9"
        self.tmp = tempfile.TemporaryDirectory()
        self.env = {"HOME": self.tmp.name, "XDG_CACHE_HOME": os.path.join(self.tmp.name, "cache"),
                    "ROKAST_UPDATE_URL": self.url, "ROKAST_CONFIG": os.path.join(self.tmp.name, "none.conf")}

    def tearDown(self):
        self.tmp.cleanup()

    def rokast(self, *args, **extra):
        env = {"PATH": os.environ["PATH"], **self.env, **extra}
        return subprocess.run([os.path.join(REPO, "scripts", "rokast"), *args], env=env,
                              capture_output=True, text=True, check=True).stdout

    def test_versions(self):
        self.assertTrue(update.is_newer("v0.10.0", "0.9.9"))
        self.assertFalse(update.is_newer("v0.2.0", "0.2.0"))
        self.assertFalse(update.is_newer("garbage", "0.2.0"))
        self.assertEqual(update.parse_version("v1.2.3-rc1"), (1, 2, 3))

    def test_newer_tag_shows_notice_once_a_day(self):
        out = self.rokast("version", "--check")
        self.assertIn("rokast 9.9.9 is available: https://example.invalid/rel", out)
        self.assertEqual(len(FakeGitHub.hits), 1)
        self.rokast("version", "--check")                      # same day: answered from the cache
        self.assertEqual(len(FakeGitHub.hits), 1)
        d = json.loads(self.rokast("version", "--check", "--json"))
        self.assertTrue(d["newer"])
        self.assertEqual((d["latest"], d["checked"]), ("9.9.9", False))

    def test_cache_expires_after_a_day(self):
        update.check("0.2.0", env=self.env, now=1000)
        update.check("0.2.0", env=self.env, now=1000 + update.DAY - 1)
        self.assertEqual(len(FakeGitHub.hits), 1)
        FakeGitHub.tag = "v0.2.0"
        res = update.check("0.2.0", env=self.env, now=1000 + update.DAY)
        self.assertEqual(len(FakeGitHub.hits), 2)
        self.assertFalse(res["newer"])

    def test_check_updates_0_makes_no_request(self):
        out = self.rokast("version", "--check", CHECK_UPDATES="0")
        self.assertIn("update check is off", out)
        with open(self.env["ROKAST_CONFIG"], "w") as f:
            f.write("CHECK_UPDATES=0\n")
        self.rokast("version", "--check", "--json")
        self.assertEqual(FakeGitHub.hits, [])
        self.assertFalse(os.path.exists(update.cache_file(self.env)))

    def test_offline_is_quiet_and_not_retried_today(self):
        env = {**self.env, "ROKAST_UPDATE_URL": "http://127.0.0.1:9/none"}
        res = update.check("0.2.0", env=env, now=5000)
        self.assertEqual((res["newer"], res["checked"]), (False, True))
        self.assertFalse(update.check("0.2.0", env=env, now=5001)["checked"])

    def test_packaged_install_points_at_package_manager(self):
        res = {"newer": True, "latest": "9.9.9", "url": "u"}
        self.assertEqual(update.message(res, "/usr/lib/rokast"), "rokast 9.9.9 is available: update with your package manager")
        self.assertEqual(update.message(res, "/home/u/.local/share/rokast/current"), "rokast 9.9.9 is available: u")
        self.assertEqual(update.message({**res, "newer": False}, "/x"), "")


class KindAndLockTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = self.tmp.name
        self.env = {"HOME": self.home, "XDG_RUNTIME_DIR": os.path.join(self.home, "run")}

    def tearDown(self):
        self.tmp.cleanup()

    def test_kind(self):
        user = os.path.join(self.home, ".local/share/rokast/versions/0.5.0")
        clone = os.path.join(self.home, "src/rokast")
        os.makedirs(user)
        os.makedirs(os.path.join(clone, ".git"))
        link = os.path.join(self.home, ".local/share/rokast/current")
        os.symlink(user, link)
        never = lambda cmd, path: False  # noqa: E731
        self.assertEqual(update.kind(user, self.env, never), "user")
        self.assertEqual(update.kind(link, self.env, never), "user")   # through the current/ link
        self.assertEqual(update.kind(clone, self.env, never), "git")
        self.assertEqual(update.kind("/usr/lib/rokast", self.env, never), "other")
        for name, tool in (("deb", "dpkg"), ("rpm", "rpm"), ("arch", "pacman")):
            self.assertEqual(update.kind("/usr/lib/rokast", self.env, lambda cmd, path, t=tool: cmd[0] == t), name)

    def test_session_lock(self):
        self.assertFalse(update.mirroring(self.env))                  # no lock file yet
        code = "import sys, time; sys.path.insert(0, sys.argv[1]); import update; f = update.hold_session(); " \
               "print('held', flush=True); time.sleep(30)"
        child = subprocess.Popen([sys.executable, "-I", "-c", code, os.path.join(REPO, "src", "miracast")],
                                 env={**os.environ, **self.env}, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(child.stdout.readline().strip(), "held")
            self.assertTrue(update.mirroring(self.env))
        finally:
            child.kill()
            child.wait()
            child.stdout.close()
        self.assertFalse(update.mirroring(self.env))                  # the lock dies with the process


if __name__ == "__main__":
    unittest.main()
