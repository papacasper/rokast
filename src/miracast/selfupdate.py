"""rokast update [--check] [--yes] [--rollback] [--auto]: replace this install with the latest signed release.

Only installs made by scripts/install (update.kind() == "user") replace themselves; packages and git clones are told
how to update instead. Steps (docs/plan-v0.5.md, phase 9):
  1. the latest release from the GitHub API (no drafts or prereleases; only if newer than this version)
  2. refuse while a `rokast start` runs (update.mirroring())
  3. download the tarball, SHA256SUMS and SHA256SUMS.sig into $XDG_CACHE_HOME/rokast/download/<v>/
  4. SHA256SUMS.sig must verify against THIS install's config/release-signers, the tarball must match SHA256SUMS
  5. extract (every member inside rokast-<v>/, no links out) and run the new version's scripts/install, which builds
     the supplicant before it switches `current` (a failed build changes nothing)
  6. check that the new version runs; if not, scripts/install --rollback
Progress goes to stdout as `update: <state> [detail]` lines (checking, uptodate, available, downloading, verifying,
installing, done, error, skipped); the app shows them. $ROKAST_UPDATE_URL (tests) also allows downloads from its
origin. --auto (the app runs it at start) installs only if auto_reason() is None, and is silent otherwise.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.parse
import urllib.request

import paths
import update

GITHUB_HOSTS = {"api.github.com", "github.com", "objects.githubusercontent.com",
                "release-assets.githubusercontent.com"}
MAX_BYTES = 200 * 1024 * 1024
HOW = {"git": "this is a git clone: update it with `git pull`",
       "arch": "installed from the AUR: update it with your AUR helper (e.g. `yay -Syu rokast`)",
       "deb": "installed from a .deb: download the new .deb from the release page and install it with apt",
       "rpm": "installed from an .rpm: download the new .rpm from the release page and install it with dnf",
       "other": "not installed by scripts/install: update it the way you installed it"}


class UpdateError(Exception):
    pass


def say(state, detail=""):
    print(f"update: {state}" + (f" {detail}" if detail else ""), flush=True)


def _origin(url):
    u = urllib.parse.urlsplit(url)
    return u.scheme, u.netloc


def allowed(url, env=os.environ):
    """HTTPS to GitHub only (plus the test server's origin when $ROKAST_UPDATE_URL is set)."""
    scheme, host = _origin(url)
    if env.get("ROKAST_UPDATE_URL") and (scheme, host) == _origin(env["ROKAST_UPDATE_URL"]):
        return True
    return scheme == "https" and host.split(":")[0] in GITHUB_HOSTS


class _CheckedRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, env):
        self.env = env

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl, self.env):
            raise UpdateError(f"refusing redirect to {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _open(url, env, timeout=30):
    if not allowed(url, env):
        raise UpdateError(f"refusing to download from {url}")
    opener = urllib.request.build_opener(_CheckedRedirect(env))
    req = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": "rokast"})
    try:
        return opener.open(req, timeout=timeout)
    except OSError as e:   # URLError, timeouts, connection refused
        raise UpdateError(f"could not reach {urllib.parse.urlsplit(url).netloc}: {getattr(e, 'reason', e)}") from None


def latest_release(env=os.environ):
    """{'version', 'assets': {name: url}, 'url'} of the latest published, non-prerelease release."""
    with _open(env.get("ROKAST_UPDATE_URL") or update.URL, env) as r:
        try:
            d = json.load(r)
        except ValueError:
            raise UpdateError("GitHub sent something that is not JSON") from None
    if d.get("draft") or d.get("prerelease") or not update.parse_version(d.get("tag_name", "")):
        raise UpdateError("the latest release is not a published version")
    return {"version": d["tag_name"].lstrip("v"), "url": d.get("html_url") or update.RELEASES,
            "assets": {a["name"]: a["browser_download_url"] for a in d.get("assets", [])}}


def download(url, dest, env=os.environ):
    with _open(url, env) as r, open(dest, "wb") as f:
        total = 0
        while chunk := r.read(1 << 16):
            total += len(chunk)
            if total > MAX_BYTES:
                raise UpdateError(f"{os.path.basename(dest)} is larger than {MAX_BYTES >> 20} MB")
            f.write(chunk)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 16), b""):
            h.update(chunk)
    return h.hexdigest()


def verify(folder, tarball, signers):
    """Signature first (so SHA256SUMS can be trusted), then the tarball's line in it."""
    sums = os.path.join(folder, "SHA256SUMS")
    if not update.verify_signature(sums, sums + ".sig", signers):
        raise UpdateError("SHA256SUMS.sig is not a valid rokast release signature")
    with open(sums) as f:
        want = {ln.split()[1].lstrip("*"): ln.split()[0] for ln in f if len(ln.split()) == 2}
    if want.get(tarball) != sha256(os.path.join(folder, tarball)):
        raise UpdateError(f"{tarball} does not match SHA256SUMS")


def extract(tarball, folder, version):
    """Extract into folder/rokast-<version>/, refusing absolute paths, '..', links out of the tree and devices."""
    top = f"rokast-{version}"
    with tarfile.open(tarball) as t:
        for m in t.getmembers():
            parts = m.name.split("/")
            if m.name.startswith("/") or ".." in parts or parts[0] != top:
                raise UpdateError(f"unsafe path in the tarball: {m.name}")
            if not (m.isfile() or m.isdir() or m.issym()):
                raise UpdateError(f"unexpected file type in the tarball: {m.name}")
            if m.issym():
                target = os.path.normpath(os.path.join(os.path.dirname(m.name), m.linkname))
                if m.linkname.startswith("/") or not target.startswith(top + "/"):
                    raise UpdateError(f"link out of the tree in the tarball: {m.name}")
        t.extractall(folder, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
    return os.path.join(folder, top)


def run_update(root, current, yes=False, check_only=False, env=os.environ, ask=input):
    """Returns the exit code. See the module docstring for the steps."""
    say("checking")
    kind = update.kind(root, env)
    rel = latest_release(env)
    if not update.is_newer(rel["version"], current):
        say("uptodate", current)
        return 0
    say("available", rel["version"])
    if kind != "user":
        say("error", HOW[kind])
        return 1
    if check_only:
        return 0
    if update.mirroring(env):
        say("error", "stop mirroring first (a rokast start is running)")
        return 1
    if not yes:
        if not sys.stdin.isatty() or ask(f"Update rokast {current} -> {rel['version']}? [y/N] ").strip().lower() != "y":
            say("error", "not confirmed (use --yes)")
            return 1
    v = rel["version"]
    tarball = f"rokast-{v}-x86_64-linux.tar.gz"
    missing = [n for n in (tarball, "SHA256SUMS", "SHA256SUMS.sig") if n not in rel["assets"]]
    if missing:
        say("error", f"release {v} has no {', '.join(missing)} (unsigned releases are not installed)")
        return 1
    folder = os.path.join(os.path.dirname(update.cache_file(env)), "download", v)
    shutil.rmtree(folder, ignore_errors=True)
    os.makedirs(folder)
    try:
        for name in ("SHA256SUMS", "SHA256SUMS.sig", tarball):
            say("downloading", name)
            download(rel["assets"][name], os.path.join(folder, name), env)
        say("verifying")
        verify(folder, tarball, os.path.join(root, "config", "release-signers"))
        new = extract(os.path.join(folder, tarball), os.path.join(folder, "x"), v)
        say("installing", v)
        return _install(new, v, env)
    except (UpdateError, OSError, tarfile.TarError) as e:
        say("error", str(e))
        return 1
    finally:
        shutil.rmtree(folder, ignore_errors=True)
        try:
            os.rmdir(os.path.dirname(folder))   # download/, if nothing else is in it
        except OSError:
            pass


def _install(new, v, env):
    base = paths.data_dir(env)
    before = os.path.realpath(os.path.join(base, "current"))
    p = subprocess.run([os.path.join(new, "scripts", "install")], capture_output=True, text=True, env=env)
    for ln in (p.stdout + p.stderr).splitlines():
        print(f"update: log {ln}", flush=True)
    if p.returncode != 0:
        raise UpdateError("the new version's installer failed; nothing changed")
    now = os.path.realpath(os.path.join(base, "current"))
    exe = os.path.join(base, "current", "scripts", "rokast")
    try:
        got = subprocess.run([exe, "version"], capture_output=True, text=True, env=env, timeout=60).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        got = ""
    if got != v:
        if now != before:
            subprocess.run([os.path.join(base, "current", "scripts", "install"), "--rollback"], env=env,
                           capture_output=True)
        raise UpdateError(f"the new version does not run (version said {got!r}); rolled back")
    say("done", v)
    return 0


def run_rollback(env=os.environ):
    if update.mirroring(env):
        say("error", "stop mirroring first (a rokast start is running)")
        return 1
    script = os.path.join(paths.data_dir(env), "current", "scripts", "install")
    if not os.path.exists(script):
        say("error", "not installed by scripts/install: nothing to roll back")
        return 1
    p = subprocess.run([script, "--rollback"], capture_output=True, text=True, env=env)
    print((p.stdout + p.stderr).strip(), flush=True)
    return p.returncode


def auto_reason(conf, kind, mirroring):
    """Why `update --auto` does nothing, or None if it may install: AUTO_UPDATE=1 (default off), update checks on,
    an install made by scripts/install, and nothing mirroring."""
    if conf.get("AUTO_UPDATE", "0") != "1":
        return "AUTO_UPDATE is off"
    if conf.get("CHECK_UPDATES", "1") == "0":
        return "CHECK_UPDATES=0"
    if kind != "user":
        return f"{kind} install"
    if mirroring:
        return "mirroring"
    return None


def main(root, current, args, conf=None, env=os.environ):
    if "--rollback" in args:
        return run_rollback(env)
    if "--auto" in args:
        why = auto_reason(conf or {}, update.kind(root, env), update.mirroring(env))
        if why:
            say("skipped", why)
            return 0
        args = [*args, "--yes"]
    try:
        return run_update(root, current, yes="--yes" in args, check_only="--check" in args, env=env)
    except UpdateError as e:
        say("error", str(e))
        return 1
