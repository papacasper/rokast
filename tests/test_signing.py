"""Release signing: scripts/sign-release (used by release.yml) and update.verify_signature (used by `rokast update`),
with a throwaway key. The real key is a repo secret; ci.yml checks it against config/release-signers."""
import os
import subprocess
import sys
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "src", "miracast"))
import update  # noqa: E402


class SigningTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = self.tmp.name
        for name in ("good", "other"):
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", self.p(name)], check=True)
        with open(self.p("good.pub")) as f:
            pub = " ".join(f.read().split()[:2])
        with open(self.p("signers"), "w") as f:
            f.write(f'rokast-release namespaces="rokast-release" {pub}\n')
        with open(self.p("SHA256SUMS"), "w") as f:
            f.write("0" * 64 + "  rokast-9.9.9-x86_64-linux.tar.gz\n")

    def tearDown(self):
        self.tmp.cleanup()

    def p(self, name):
        return os.path.join(self.d, name)

    def sign(self, key):
        with open(self.p(key)) as f:
            env = {"PATH": os.environ["PATH"], "ROKAST_SIGNING_KEY": f.read(), "ROKAST_SIGNERS": self.p("signers")}
        return subprocess.run([os.path.join(REPO, "scripts", "sign-release"), self.p("SHA256SUMS")], env=env,
                              capture_output=True, text=True)

    def test_sign_and_verify(self):
        self.assertEqual(self.sign("good").returncode, 0)
        self.assertTrue(update.verify_signature(self.p("SHA256SUMS"), self.p("SHA256SUMS.sig"), self.p("signers")))

    def test_one_changed_byte_is_rejected(self):
        self.sign("good")
        with open(self.p("SHA256SUMS"), "r+b") as f:
            f.write(b"1")
        self.assertFalse(update.verify_signature(self.p("SHA256SUMS"), self.p("SHA256SUMS.sig"), self.p("signers")))

    def test_unlisted_key_fails_to_sign_and_verify(self):
        r = self.sign("other")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not in", r.stderr)
        self.assertFalse(update.verify_signature(self.p("SHA256SUMS"), self.p("SHA256SUMS.sig"), self.p("signers")))

    def test_missing_signature_or_key(self):
        self.assertFalse(update.verify_signature(self.p("SHA256SUMS"), self.p("nope.sig"), self.p("signers")))
        r = subprocess.run([os.path.join(REPO, "scripts", "sign-release"), self.p("SHA256SUMS")],
                           env={"PATH": os.environ["PATH"]}, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)

    def test_shipped_signers_file_is_valid(self):
        with open(os.path.join(REPO, "config", "release-signers")) as f:
            lines = [ln.split() for ln in f if ln.strip() and not ln.startswith("#")]
        self.assertTrue(lines)
        for ln in lines:
            self.assertEqual(ln[:3], ["rokast-release", 'namespaces="rokast-release"', "ssh-ed25519"])


if __name__ == "__main__":
    unittest.main()
