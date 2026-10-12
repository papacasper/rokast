"""Update notice: is a newer rokast release out?  Never downloads anything.

At most one request a day to GitHub's latest-release API; the answer is cached in $XDG_CACHE_HOME/rokast/update.json
(the attempt time is stored even when offline, so a dead network is not retried on every start).
CHECK_UPDATES=0 in rokast.conf (or the environment) means no request at all. $ROKAST_UPDATE_URL overrides the URL (tests).
Packaged installs (/usr/...) are told to update with their package manager instead of linking a tarball.
kind() says how this copy was installed; mirroring() whether a `rokast start` is running (its session lock);
verify_signature() checks a release file against config/release-signers (docs/plan-v0.5.md).
"""
import fcntl
import json
import os
import subprocess
import time
import urllib.request

URL = "https://api.github.com/repos/papacasper/rokast/releases/latest"
RELEASES = "https://github.com/papacasper/rokast/releases/latest"
DAY = 24 * 3600


def parse_version(v):
    """'v0.10.2' -> (0, 10, 2); anything unparsable -> () (never "newer")."""
    try:
        return tuple(int(x) for x in v.strip().lstrip("v").split("-")[0].split("."))
    except (ValueError, AttributeError):
        return ()


def is_newer(latest, current):
    a, b = parse_version(latest), parse_version(current)
    return bool(a) and bool(b) and a > b


def cache_file(env=os.environ):
    base = env.get("XDG_CACHE_HOME") or os.path.join(env.get("HOME") or os.path.expanduser("~"), ".cache")
    return os.path.join(base, "rokast", "update.json")


def _read(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def fetch(url, timeout=5):
    """(tag, page url) of the latest release, or None on any failure."""
    req = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": "rokast"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            d = json.load(r)
        return d["tag_name"], d.get("html_url") or RELEASES
    except Exception:  # noqa: BLE001 - offline, rate-limited, bad JSON: no notice, try again tomorrow
        return None


def check(current, enabled=True, env=os.environ, now=None, fetcher=fetch):
    """{'current', 'latest', 'newer', 'url', 'checked'}; makes a request only when enabled and the cache is a day old."""
    out = {"current": current, "latest": None, "newer": False, "url": RELEASES, "checked": False, "enabled": enabled}
    if not enabled:
        return out
    now = time.time() if now is None else now
    path = cache_file(env)
    c = _read(path)
    if not isinstance(c.get("at"), (int, float)) or not 0 <= now - c["at"] < DAY:
        got = fetcher(env.get("ROKAST_UPDATE_URL") or URL)
        c = {"at": now, "latest": got[0] if got else c.get("latest"), "url": got[1] if got else c.get("url")}
        out["checked"] = True
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                json.dump(c, f)
        except OSError:
            pass
    if c.get("latest"):
        out.update(latest=c["latest"].lstrip("v"), url=c.get("url") or RELEASES, newer=is_newer(c["latest"], current))
    return out


def packaged(root):
    return root.startswith("/usr/")


def _owns(cmd, path):
    try:
        return subprocess.run([*cmd, path], capture_output=True, timeout=10).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def kind(root, env=os.environ, owns=_owns):
    """How this copy was installed: 'user' (scripts/install, under $XDG_DATA_HOME/rokast/versions/), 'deb', 'rpm',
    'arch' (a package owns the folder), 'git' (a clone), or 'other'. Only 'user' may replace itself."""
    real = os.path.realpath(root)
    data = env.get("XDG_DATA_HOME") or os.path.join(env.get("HOME") or os.path.expanduser("~"), ".local", "share")
    if real.startswith(os.path.realpath(os.path.join(data, "rokast", "versions")) + os.sep):
        return "user"
    if os.path.exists(os.path.join(real, ".git")):
        return "git"
    for name, cmd in (("deb", ["dpkg", "-S"]), ("rpm", ["rpm", "-qf"]), ("arch", ["pacman", "-Qo"])):
        if owns(cmd, real):
            return name
    return "other"


def lock_file(env=os.environ):
    """The session lock `rokast start` holds while it runs."""
    base = env.get("XDG_RUNTIME_DIR") or os.path.dirname(os.path.dirname(cache_file(env)))
    return os.path.join(base, "rokast", "session.lock")


def hold_session(env=os.environ):
    """Take the session lock; keep the returned file open for as long as the session runs."""
    path = lock_file(env)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    f = open(path, "a")
    try:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        pass   # another session runs; it fails on its own (one P2P link) and the lock stays held either way
    return f


def mirroring(env=os.environ):
    """True while some `rokast start` holds the session lock."""
    try:
        with open(lock_file(env)) as f:
            fcntl.flock(f, fcntl.LOCK_SH | fcntl.LOCK_NB)
            return False
    except FileNotFoundError:
        return False
    except OSError:
        return True


def verify_signature(path, sig, signers):
    """True if `sig` is a valid rokast-release signature of `path` by a key in `signers` (ssh-keygen -Y verify)."""
    try:
        with open(path, "rb") as f:
            return subprocess.run(["ssh-keygen", "-q", "-Y", "verify", "-f", signers, "-I", "rokast-release",
                                   "-n", "rokast-release", "-s", sig], stdin=f, capture_output=True,
                                  timeout=30).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def message(res, root):
    """One line for people, or '' when there is nothing to say."""
    if not res["newer"]:
        return ""
    how = "update with your package manager" if packaged(root) else res["url"]
    return f"rokast {res['latest']} is available: {how}"
