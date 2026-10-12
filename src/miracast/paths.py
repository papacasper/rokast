"""Where rokast keeps things, independent of where it is installed (so upgrades don't lose them).

  config      $ROKAST_CONFIG, else $XDG_CONFIG_HOME/rokast/rokast.conf   (legacy: <install>/config/rokast.conf)
  supplicant  $ROKAST_SUPPLICANT, else $XDG_DATA_HOME/rokast/supplicant/wpa_supplicant, else the packaged
              /usr/lib/rokast/wpa_supplicant                              (legacy: <install>/vendor/hostap-src/...)
  build tree  $XDG_CACHE_HOME/rokast/hostap-src (scripts/build-supplicant)

Everything takes `env` so it is testable without touching the real home directory.
scripts/build-supplicant and ui/src/config.rs follow the same rules; keep them in step.
"""
import hashlib
import os
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PACKAGED_SUPPLICANT = "/usr/lib/rokast/wpa_supplicant"
PATCH_FILES = ("patches/hostap-2.11-roku.patch", "patches/hostap.config")


def _xdg(env, var, fallback):
    return env.get(var) or os.path.join(env.get("HOME") or os.path.expanduser("~"), fallback)


def config_dir(env=os.environ):
    return os.path.join(_xdg(env, "XDG_CONFIG_HOME", ".config"), "rokast")


def data_dir(env=os.environ):
    return os.path.join(_xdg(env, "XDG_DATA_HOME", ".local/share"), "rokast")


def legacy_config(root=ROOT):
    return os.path.join(root, "config", "rokast.conf")


def config_file(env=os.environ, root=ROOT):
    """(file to read, legacy file to migrate into it or None)."""
    if env.get("ROKAST_CONFIG"):
        return env["ROKAST_CONFIG"], None
    path = os.path.join(config_dir(env), "rokast.conf")
    legacy = legacy_config(root)
    return path, (legacy if not os.path.exists(path) and os.path.isfile(legacy) else None)


def migrate_config(path, legacy):
    """Copy the pre-0.2 in-tree config to its new home; rename the old one so nobody edits a dead file."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    shutil.copy2(legacy, path)
    os.replace(legacy, legacy + ".moved")
    print(f"config: moved {legacy} -> {path}", file=sys.stderr)


def supplicant_candidates(env=os.environ, root=ROOT):
    if env.get("ROKAST_SUPPLICANT"):
        return [env["ROKAST_SUPPLICANT"]]
    return [os.path.join(data_dir(env), "supplicant", "wpa_supplicant"), PACKAGED_SUPPLICANT,
            os.path.join(root, "vendor", "hostap-src", "wpa_supplicant", "wpa_supplicant")]


def find_supplicant(env=os.environ, root=ROOT, usable=lambda p: os.access(p, os.X_OK)):
    """First usable patched wpa_supplicant, or None."""
    return next((p for p in supplicant_candidates(env, root) if usable(p)), None)


def patch_hash(root=ROOT):
    """sha256 of the shipped patch + build config; scripts/build-supplicant stores it as patch.sha256."""
    h = hashlib.sha256()
    for f in PATCH_FILES:
        with open(os.path.join(root, f), "rb") as fh:
            h.update(fh.read())
    return h.hexdigest()


def supplicant_state(path, root=ROOT):
    """'missing' | 'ok' | 'outdated' (built from a different patch) | 'legacy' (old in-tree location) | 'external'."""
    if not path:
        return "missing"
    if path.startswith(os.path.join(root, "vendor") + os.sep):
        return "legacy"
    stamp = os.path.join(os.path.dirname(path), "patch.sha256")
    if not os.path.isfile(stamp):
        return "external"     # packaged or $ROKAST_SUPPLICANT: managed elsewhere
    with open(stamp) as f:
        return "ok" if f.read().strip() == patch_hash(root) else "outdated"
