# Plan: v0.5.0, self-updater

Status: decisions approved 2026-10-12; phase 8 done (key created, CI checks it). Follows `docs/plan-v0.2.md` (phases 0-6). Same rules as there (compatibility, tests,
files under 500 lines, live check after `p2p`/`wfd`/`capture`/`cli` changes, no AI attribution). Each task has a
**Done when** check that must pass by running it, not by eye. Tick tasks off here as they land.

| Phase | What | Release | Who |
|---|---|---|---|
| 7 | Ship what is merged (phase 6) | v0.4.0 | user login check, then Claude |
| 8 | Signed releases + install-kind detection | (no release) | Claude, user approves the signing key |
| 9 | `rokast update` for user installs | v0.5.0 | Claude |
| 10 | Update button in the app, package installs | v0.5.0 | Claude |
| 11 | Tests, rehearsal against a real release, release v0.5.0 | v0.5.0 | Claude, user confirms TV |

## What "self-updater" means here

rokast is installed four ways, and only one of them may replace itself:

| Install | Where | Update path in v0.5.0 |
|---|---|---|
| Release tarball + `scripts/install` | `~/.local/share/rokast/versions/<v>`, `current` link | **self-update**: download, verify, install, switch `current`, keep `previous` for rollback |
| .deb / .rpm downloaded from a release | `/usr/lib/rokast` (dpkg/rpm own it) | download and verify the new package, then hand it to the package manager through `pkexec` (the polkit password prompt is the user's consent). Never write to `/usr` ourselves |
| AUR (`rokast`, `rokast-git`) | `/usr/lib/rokast` (pacman owns it) | no download: tell them to update with their AUR helper (pacman installs must stay pacman's) |
| git clone (`scripts/rokast` from the repo) | the clone | no download: "run `git pull`" |

Why not apt/dnf repositories or COPR: a real package repository would give .deb/.rpm users normal system updates,
but it means hosting and signing a repo. Not in v0.5.0; revisit if people ask.

Trust: today the only thing protecting a download is HTTPS to GitHub. The updater installs code that runs with
`sudo` (the supplicant), so releases get signed and the updater refuses anything it cannot verify against a public
key it already has installed. The signature check runs in the **installed** (old) version; the installer that
does the work comes from the **new** version, so installer fixes ship with the update.

Limit: the updater can only update from a version that has it. Everyone on v0.4.0 or older updates by hand one last
time (CHANGELOG "Upgrading" note). The first real self-update is v0.5.0 -> the next release.

## Decisions (approved by the user 2026-10-12)

1. **Signing:** an `ssh-keygen -Y sign` key (OpenSSH is on every target distro, so no new dependency). The private
   key goes in the repo secret `ROKAST_SIGNING_KEY` and a backup in the vault; the public key ships in the repo as
   `config/release-signers` (allowed_signers format). Created 2026-10-12, fingerprint
   `SHA256:JIwe7G1Gp6TgOqCR6achVy0q5JPaRC/yEaVopRCYbFg`; vault note "rokast release signing key".
2. **When updates install:** only when the user clicks **Update** (app) or runs `rokast update`. A setting
   `AUTO_UPDATE=1` (default **off**) installs in the background at app start when not mirroring.
3. **.deb/.rpm through `pkexec`:** yes, as in the table above. If polkit is missing, print the exact command instead.

---

## Phase 7: release v0.4.0

Why: phase 6 (join fix, retry, Fedora .rpm, libopenh264, Wayland CI) is merged but unreleased. Release it before
starting a feature that changes the release pipeline.

- [ ] **Real-login autostart check** (user): log out and in; the tray icon appears, no window.
- [ ] **Release v0.4.0** by `playbooks/release.md` (CHANGELOG `## Unreleased` -> `## 0.4.0`). AUR push only with the
      user's go-ahead.
      **Done when:** the playbook's Verify section passes, including the .rpm in the asset list.
- [ ] Optional: the full soak (`tests/live_join_soak.sh 10 15`) to close phase 6's "Reproduce it" task.

## Phase 8: signed releases + install-kind detection

- [x] **Sign every release.** `release.yml` signs `SHA256SUMS` with `ssh-keygen -Y sign -n rokast-release` and
      publishes `SHA256SUMS.sig`; it fails if the secret is missing. The public key is in `config/release-signers`.
      Key rotation: a release may list two keys in `config/release-signers`; the new key starts signing only after a
      release that ships it, signed by the old key. `playbooks/release.md` gets a "check the signature" step.
      **Done when:** a test signs a file with a throwaway key and `ssh-keygen -Y verify` accepts it and rejects a
      one-byte change; the release workflow's sign step runs on a test tag in a fork or a dry-run job.
      Done: `scripts/sign-release`, `tests/test_signing.py`; ci.yml signs a dry-run file with the real secret on every
      push. Lands before v0.4.0, so v0.4.0 is already signed. `packaging/stage` ships the signers file; the .deb/.rpm
      tests check it and the install kind.
- [x] **Install kind.** `update.kind(root)` -> `user` (under `$XDG_DATA_HOME/rokast/versions/`), `deb`/`rpm`/`arch`
      (`dpkg -S`, `rpm -qf`, `pacman -Qo` on `/usr/lib/rokast`), `git` (has `.git`), `other`. Replaces `packaged()`;
      `rokast version --check --json` reports `kind` (keep `packaged` for the v0.4 app until v0.6).
      **Done when:** unit tests cover each kind with stubbed commands and paths.
- [x] **Session lock.** `rokast start` holds `flock` on `$XDG_RUNTIME_DIR/rokast/session.lock` while it runs;
      `update.mirroring()` checks it. Needed so nothing updates under a live mirror.
      **Done when:** a unit test holds the lock in a child process and `mirroring()` is true, false after it exits.

## Phase 9: `rokast update` (user installs)

- [ ] **`rokast update [--check] [--yes] [--rollback]`** in a new `src/miracast/selfupdate.py`:
      1. Latest release from the GitHub API (reuse `update.fetch`, now returning the assets); skip drafts and
         prereleases; do nothing unless it is newer. `--check` stops here and prints what it would do.
      2. Refuse while mirroring (session lock), and for `git`/`arch`/`other` installs (print their update path).
      3. Download the tarball, `SHA256SUMS` and `SHA256SUMS.sig` to `$XDG_CACHE_HOME/rokast/download/<v>/`: HTTPS
         only, GitHub hosts only, size cap, timeout.
      4. Verify the signature against the installed `config/release-signers`, then the tarball's sha256. Any
         failure: delete the download, change nothing, say why.
      5. Extract safely (every member under `rokast-<v>/`, no links out, no absolute paths), then run the new
         version's `scripts/install`.
      6. Check: `~/.local/bin/rokast version` prints the new version and doctor's supplicant line is ok. If not,
         `scripts/install --rollback` and report. Delete the download either way.
      Without `--yes` it asks before step 3. Progress goes out as `update: <step> ...` lines (new prefix; `status:`
      lines are unchanged) so the app can show them. `--rollback` runs `scripts/install --rollback`.
      **Done when:** `tests/test_self_update.sh` (below) passes.
- [ ] **Installer: build before switching.** Today `scripts/install` switches `current`, then builds the supplicant;
      a failed build leaves the new version active with an outdated supplicant. Build into the new version's slot
      first, switch only on success.
      **Done when:** `tests/test_install.sh` has a case with a failing build stub: `current` still points at the old
      version and the exit code is non-zero.
- [ ] **AUTO_UPDATE=1** (default off, `rokast.conf`): the app runs `rokast update --yes` at start when an update is
      known, nothing is mirroring and the kind is `user`. Never for packages.
      **Done when:** a unit test covers the decision (on/off x mirroring x kind); README config table lists it.

## Phase 10: the app, and package installs

- [ ] **Update button.** The banner becomes "rokast X.Y.Z is available [Update]" for `user` installs; clicking runs
      `rokast update --yes` and shows its `update:` lines in the banner. The button is disabled while mirroring.
      Success: "Updated to X.Y.Z [Restart rokast]" (re-runs `current/.../rokast-ui`, with `--tray` if it was
      started that way). Failure: the reason plus the "Get it" link. The tray menu item becomes "Update to X.Y.Z".
      **Done when:** Rust unit tests cover the banner states (available, updating, done, failed, packaged per kind);
      the app is checked against the fake release server from the tests (screenshots in the session note).
- [ ] **.deb/.rpm.** `rokast update` downloads and verifies the matching `.deb`/`.rpm` (same signature + sha256
      checks), then runs `pkexec apt-get install -y ./rokast_X_amd64.deb` or `pkexec dnf install -y ./rokast-X.rpm`.
      No polkit agent or no `pkexec`: print the command for the user to run. AUR/git: print their update path.
      **Done when:** `tests/test_pkg_deb.sh` and `tests/test_pkg_rpm.sh` gain an upgrade case: install a package
      built at version A, serve version B from the fake server, `rokast update --yes` (as root, so no pkexec)
      leaves version B installed.

## Phase 11: tests, rehearsal, release

- [ ] **`tests/test_self_update.sh`** (no docker; scratch `HOME`; runs in CI): build two release tarballs from the
      working tree (`VERSION` A and B), sign `SHA256SUMS` with a throwaway key listed in a test
      `config/release-signers`, serve them from a local HTTP server (`ROKAST_UPDATE_URL`), install A, then check:
      - `rokast update --yes` -> `current` is B, `previous` is A, settings unchanged, the download dir is gone;
      - `rokast update --rollback` -> `current` is A;
      - a tampered tarball, a tampered `SHA256SUMS`, a missing or wrong signature -> refused, `current` unchanged;
      - an older or equal version, a prerelease -> nothing to do;
      - session lock held -> refused; `git` install -> "run git pull", no download;
      - server down -> clear error, nothing changed.
      **Done when:** it passes locally and in `ci.yml`.
- [ ] **Rehearsal against the real release.** After v0.5.0 is published: install HEAD with `VERSION=0.4.99` in a
      scratch `HOME`, `rokast update --yes` -> 0.5.0 from GitHub, signature verified, rollback works. Then the live
      check on this laptop (the updater touches `cli.py`).
      **Done when:** both pass; results in the session note.
- [ ] **Docs.** README "Updating" section (the table above, in user words); config table (`AUTO_UPDATE`);
      CHANGELOG with an "Upgrading" note (from v0.4.0 or older, update by hand once); `playbooks/release.md`
      (signing and the `.sig` asset in Verify).
- [ ] **Release v0.5.0** by `playbooks/release.md`.
      **Done when:** Verify passes, `SHA256SUMS.sig` is an asset, and `ssh-keygen -Y verify` accepts it.

## Not planned

Delta updates, a background update service, updating the supplicant separately from rokast, apt/dnf repositories or
COPR/OBS (see above), Flatpak.
