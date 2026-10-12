"""Path rules (src/miracast/paths.py): config location + migration, supplicant lookup + patch staleness."""
import contextlib
import io
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "src", "miracast"))
import cli  # noqa: E402
import paths  # noqa: E402


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="rokast-test-")
        self.addCleanup(shutil.rmtree, self.tmp)
        self.home = os.path.join(self.tmp, "home")
        self.root = os.path.join(self.tmp, "install")
        os.makedirs(os.path.join(self.root, "config"))
        os.makedirs(self.home)
        self.env = {"HOME": self.home}

    def write(self, path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)


class Config(Base):
    def test_fresh_install_uses_xdg_path_and_nothing_to_migrate(self):
        path, legacy = paths.config_file(self.env, self.root)
        self.assertEqual(path, os.path.join(self.home, ".config", "rokast", "rokast.conf"))
        self.assertIsNone(legacy)

    def test_xdg_config_home_is_honoured(self):
        env = dict(self.env, XDG_CONFIG_HOME=os.path.join(self.tmp, "xdg"))
        self.assertEqual(paths.config_file(env, self.root)[0], os.path.join(self.tmp, "xdg", "rokast", "rokast.conf"))

    def test_legacy_only_is_migrated_once(self):
        legacy = paths.legacy_config(self.root)
        self.write(legacy, "TARGET=Den\n")
        path, todo = paths.config_file(self.env, self.root)
        self.assertEqual(todo, legacy)
        with contextlib.redirect_stderr(io.StringIO()) as err:
            paths.migrate_config(path, todo)
        self.assertIn("moved", err.getvalue())
        self.assertEqual(open(path).read(), "TARGET=Den\n")
        self.assertFalse(os.path.exists(legacy))
        self.assertTrue(os.path.exists(legacy + ".moved"))
        self.assertIsNone(paths.config_file(self.env, self.root)[1])

    def test_new_location_wins_over_legacy(self):
        self.write(paths.legacy_config(self.root), "TARGET=Old\n")
        self.write(os.path.join(self.home, ".config", "rokast", "rokast.conf"), "TARGET=New\n")
        self.assertIsNone(paths.config_file(self.env, self.root)[1])

    def test_rokast_config_env_overrides_and_never_migrates(self):
        self.write(paths.legacy_config(self.root), "TARGET=Old\n")
        self.assertEqual(paths.config_file(dict(self.env, ROKAST_CONFIG="/x/y.conf"), self.root), ("/x/y.conf", None))

    def test_load_conf_reads_file_and_env_beats_file(self):
        conf_path = os.path.join(self.tmp, "c.conf")
        self.write(conf_path, "TARGET=Den  # comment\nBITRATE=4M\nNAME=Box\n")
        old = dict(os.environ)
        self.addCleanup(lambda: (os.environ.clear(), os.environ.update(old)))
        for k in ("TARGET", "BITRATE", "NAME"):
            os.environ.pop(k, None)
        os.environ.update(ROKAST_CONFIG=conf_path, BITRATE="12M")
        conf = cli.load_conf()
        self.assertEqual((conf["TARGET"], conf["BITRATE"], conf["NAME"], conf["_file"]), ("Den", "12M", "Box", conf_path))


class Supplicant(Base):
    def test_lookup_order(self):
        c = paths.supplicant_candidates(self.env, self.root)
        self.assertEqual(c[0], os.path.join(self.home, ".local", "share", "rokast", "supplicant", "wpa_supplicant"))
        self.assertEqual(c[1], paths.PACKAGED_SUPPLICANT)
        self.assertTrue(c[2].startswith(os.path.join(self.root, "vendor")))
        self.assertEqual(paths.supplicant_candidates(dict(self.env, ROKAST_SUPPLICANT="/s"), self.root), ["/s"])

    def test_find_picks_first_usable(self):
        c = paths.supplicant_candidates(self.env, self.root)
        self.assertEqual(paths.find_supplicant(self.env, self.root, usable=lambda p: p in c[1:]), c[1])
        self.assertIsNone(paths.find_supplicant(self.env, self.root, usable=lambda p: False))

    def test_state_tracks_patch_hash(self):
        for f in paths.PATCH_FILES:
            self.write(os.path.join(self.root, f), f"contents of {f}\n")
        binary = os.path.join(self.home, "sup", "wpa_supplicant")
        self.write(binary, "")
        self.assertEqual(paths.supplicant_state(binary, self.root), "external")       # no stamp
        self.write(os.path.join(self.home, "sup", "patch.sha256"), paths.patch_hash(self.root) + "\n")
        self.assertEqual(paths.supplicant_state(binary, self.root), "ok")
        self.write(os.path.join(self.root, paths.PATCH_FILES[0]), "a changed patch\n")
        self.assertEqual(paths.supplicant_state(binary, self.root), "outdated")
        self.assertEqual(paths.supplicant_state(None, self.root), "missing")
        legacy = os.path.join(self.root, "vendor", "hostap-src", "wpa_supplicant", "wpa_supplicant")
        self.assertEqual(paths.supplicant_state(legacy, self.root), "legacy")

    def test_patch_hash_matches_build_script(self):
        # build-supplicant stores `cat patch config | sha256sum`; paths.patch_hash must give the same digest
        import hashlib
        data = b"".join(open(os.path.join(HERE, f), "rb").read() for f in paths.PATCH_FILES)
        self.assertEqual(paths.patch_hash(HERE), hashlib.sha256(data).hexdigest())


if __name__ == "__main__":
    unittest.main()
