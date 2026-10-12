# Plan: v0.2.0 and v0.3.0

Status: proposed 2026-10-12; phases 0, 1 and 2 done 2026-10-12. Each task has a **Done when** check that must pass by running it, not by eye.
Order matters: each phase relies on the one before it. Tick tasks off here as they land; move the
finished work into `CHANGELOG.md` at release time (`playbooks/release.md`).

| Phase | What | Release | Who |
|---|---|---|---|
| 0 | CI on every push | (no release) | Claude |
| 1 | Upgrade-safe install (config, supplicant, installer) | v0.2.0 | Claude |
| 2 | Capture self-test + the untested paths | v0.2.0 | Claude + test machines |
| 3 | Small features (autostart, monitor choice) | v0.2.0 | Claude |
| 4 | Release v0.2.0 + upgrade test from v0.1.0 | v0.2.0 | Claude, user confirms TV |
| 5 | Packaging (AUR, then .deb) + update notice | v0.3.0 | Claude, user owns AUR account |

## Rules for every phase

- **Compatibility:** config key names, `status:` line names, the `rokast` subcommands and the `--tray` flag stay as they are.
  Anything renamed keeps the old name working for one release and gets an "Upgrading" note in the CHANGELOG.
- **Tests:** a code change ships with a unit test where the logic is pure (path resolution, config migration, command builders).
- **Limits:** files under 500 lines; no AI attribution in commits, tags or release notes.
- **Live check:** after any change to `p2p.py`, `wfd.py`, `capture.py` or `cli.py`: mirror to the Living Room TV
  (`curl -s 192.168.0.63:8060/query/active-app` shows RokuCast), stop, then `iw dev` lists only `wlo1`.

---

## Phase 0: CI on every push

Why: today tests only run when a release is tagged, so a broken `main` is found at release time.

- [x] `.github/workflows/ci.yml` on push + pull_request: Python tests (3.10 and 3.12, the oldest and newest we
      claim), `cargo build --locked` + `cargo test` on ubuntu-22.04, `bash -n scripts/*`, `shellcheck scripts/*`.
- [x] README badge.

**Done when:** `gh run list -w ci -L1` shows `completed success` on the commit that adds it, and a deliberately broken
test on a throwaway branch shows `failure`.

## Phase 1: Upgrade-safe install

Why: with the v0.1.0 tarball, unpacking a new version leaves the user's settings, patched supplicant, menu entry and
PATH link behind in the old folder.

### 1a. Config moves to `~/.config/rokast/rokast.conf`
- [x] `cli.py load_conf()`: read `$XDG_CONFIG_HOME/rokast/rokast.conf`; if missing and `<install>/config/rokast.conf`
      exists, copy it there once and print `config: moved to ...`. Env vars still override.
- [x] `ROKAST_CONFIG=<file>` env override (tests, multiple setups).
- [x] `ui/src/config.rs conf_target()`: same lookup (or ask the CLI: `rokast config get TARGET`, so the lookup lives in one place).
- [x] `rokast doctor` prints which config file it used.
- [x] Tests: fresh (no file) -> defaults; legacy only -> migrated; both -> XDG wins; env beats file.

**Done when:** tests pass; on this laptop `rokast doctor` reports `~/.config/rokast/rokast.conf` with `TARGET=Living Room`
after one run, and the repo's `config/rokast.conf` is no longer read.

### 1b. Patched supplicant moves out of the install folder
- [x] `build-supplicant` installs the binary to `~/.local/share/rokast/supplicant/wpa_supplicant` plus `patch.sha256`
      (sha256 of `patches/hostap-2.11-roku.patch` + `patches/hostap.config`). Build tree stays in a cache dir
      (`~/.cache/rokast/hostap-src`), not `vendor/`.
- [x] `p2p.py` lookup order: `$ROKAST_SUPPLICANT` -> `~/.local/share/rokast/supplicant/` -> `/usr/lib/rokast/wpa_supplicant`
      (packages, phase 5) -> legacy `vendor/...`. One pure function, unit-tested.
- [x] `doctor`: `FAIL patched wpa_supplicant is out of date (patch changed): run scripts/build-supplicant` when the
      stored hash differs from the shipped patch.

**Done when:** tests pass; after `scripts/build-supplicant`, `rm -rf vendor/` and a live mirror still works; changing one
byte of a copy of the patch makes `doctor` report out of date.

### 1c. Installer with versioned installs
- [x] `scripts/install`: copy the release into `~/.local/share/rokast/versions/<VERSION>/`, point
      `~/.local/share/rokast/current` at it, write `~/.local/bin/rokast` (symlink to `current/scripts/rokast`) and the menu
      entry with `Exec=.../current/ui/target/release/rokast-ui`. Runs `install-ui` logic (prebuilt or cargo).
      Builds the supplicant only if missing or out of date (1b hash).
- [x] `scripts/install --rollback`: point `current` at the previous version. `--uninstall`: remove all but config.
- [x] Keep the last 2 versions, delete older ones.
- [x] Running straight from a git clone keeps working (dev mode, no install needed).

**Done when:** in a scratch `HOME`: install v0.1.0 layout -> install new build -> `rokast version` shows the new one, the
menu entry still launches, config kept; `--rollback` -> old version; `--uninstall` leaves only `~/.config/rokast/`.
Scripted as `tests/test_install.sh`, run in CI.

## Phase 2: Capture self-test + the untested paths

Why: GNOME/KDE Wayland, VAAPI and "no tray" have never run. Capture can be tested without a Roku or Wi-Fi Direct,
which makes these testable on any machine, VM or live USB.

- [x] `rokast selftest [--seconds 5] [--out file.ts]`: runs the real capture + encode pipeline (same `capture.py`
      code, same encoder pick) into a file instead of RTP, then checks it with `ffprobe` (H.264 stream at the expected
      size, at least `fps * seconds * 0.8` frames, audio stream if AUDIO=1). Prints `selftest: ok ...` / exits non-zero.
- [x] Hardware report: `rokast doctor --report` prints doctor + selftest + distro, desktop, session type, GPU, Wi-Fi chip,
      kernel. Issue template `.github/ISSUE_TEMPLATE/hardware-report.md` asks for it.
- [x] Test matrix (record results in README "Status / compatibility"). 2026-10-12: X11+NVENC, X11+VAAPI Intel
      (needed intel-media-driver; found and fixed two VAAPI argv bugs), libx264, no tray host: tested here. GNOME, KDE,
      AMD and another Wi-Fi chip: listed as untested, waiting for hardware reports.

| Path | How to test | Needs |
|---|---|---|
| GNOME Wayland portal | Fedora/Ubuntu live USB or VM, `rokast selftest` | a VM or spare machine |
| KDE Wayland portal | KDE neon / Fedora KDE live USB or VM, `rokast selftest` | a VM or spare machine |
| VAAPI (Intel) | any Intel laptop, `ENCODER=h264_vaapi rokast selftest` | Intel machine |
| VAAPI (AMD) | any AMD GPU, same | AMD machine |
| Full mirror on another chip | `rokast start` on a non-AX211 laptop | that laptop + a Roku |
| No tray host | here: run the UI in `dbus-run-session` (no StatusNotifierWatcher on that bus) | nothing |

**Done when:** `rokast selftest` passes here on X11 + NVENC; the no-tray run shows the window and closing it exits
(process gone, `iw dev` only wlo1); each matrix row is ticked with distro/version or listed as untested in the README.

## Phase 3: Small features

- [x] **Autostart toggle** in the app ("Start with my computer, in the tray"): writes/removes
      `~/.config/autostart/io.github.papacasper.rokast.desktop` with `Exec=... --tray`. Off by default.
      **Done when:** toggling creates/removes the file; after logout/login the tray icon is there and no window.
- [x] **Monitor choice on X11:** `MONITOR=` config key (name from `xrandr --listmonitors`, empty = primary) and a
      dropdown in the app. Capture that monitor's geometry, scaled/letterboxed to the TV mode. Pure function
      `monitor_geometry(xrandr_output, name)` unit-tested. (Wayland already lets the user pick in the portal dialog.)
      **Done when:** tests pass; selftest output from a second monitor shows that monitor's content (check a frame).
- [x] **Remember the window size/position**, and the Details pane open/closed state. Optional.
      (Size and Details only: GTK 4 cannot place windows; the window manager decides the position.)

Phase 3 status: done. Autostart file on/off, monitor dropdown and remembered size/Details checked in the app
(isolated config, `dbus-run-session`); `tests/test_monitor_capture.sh` checks that each monitor's own content is captured;
live mirror to the Living Room TV with the new monitor capture works. Not yet checked: an actual logout/login with
autostart on.

## Phase 4: Release v0.2.0

- [ ] CHANGELOG `## 0.2.0` with an **Upgrading from 0.1.0** section: config moves automatically; run
      `scripts/install` from the new tarball; run `scripts/build-supplicant` once more (new location); old folder can be deleted.
- [ ] Upgrade test on this laptop, the way a user would: v0.1.0 tarball installed and configured -> v0.2.0 tarball ->
      `scripts/install` -> `rokast doctor` all ok, TARGET kept, menu entry opens v0.2.0, live mirror works.
- [ ] `scripts/release 0.2.0`, then the checks in `playbooks/release.md`.

**Done when:** release workflow green, assets verified per playbook, upgrade test passed, TV shows RokuCast from v0.2.0.

## Phase 5 (v0.3.0): Packaging + update notice

- [ ] **AUR `rokast`** (release tarball) and **`rokast-git`**: `build()` builds the UI and the patched supplicant;
      install to `/usr/lib/rokast/` (supplicant found via the 1b lookup), `/usr/bin/rokast`, desktop file, icon.
      Depends: ffmpeg iw dhcpcd wpa_supplicant libnl openssl python gtk4; optdepends for the Wayland bits.
      `namcap` clean. Needs the user's AUR account + SSH key (Claude prepares, user approves the push).
      **Done when:** `makepkg -si` in a clean chroot (`extra-x86_64-build`) works and `rokast doctor` passes from `/usr/bin`.
- [ ] **.deb** built in the release workflow (same layout), attached to the release. **Done when:** installs and
      `rokast doctor` runs in an `ubuntu:22.04` container.
- [ ] **Update notice:** at most once a day, the app (and `rokast version --check`) asks
      `api.github.com/repos/papacasper/rokast/releases/latest`; if newer, a small banner/tray menu line "rokast X.Y.Z is
      available" linking to the release. `CHECK_UPDATES=0` turns it off; never auto-downloads. Packaged installs say
      "update with your package manager". **Done when:** a unit test with a fake newer tag shows the notice, and with
      `CHECK_UPDATES=0` no request is made (checked by pointing it at a local server that records hits).
- [ ] App icon: a real `rokast.svg` instead of the theme's `video-display`.

## Not planned (and why)

- Shipping a prebuilt patched `wpa_supplicant` in tarballs: it links the system's libnl/OpenSSL; building locally is safer.
  Packages (phase 5) build it on the user's machine anyway.
- Non-Roku sinks as a supported target: welcome as reports, but no hardware here to keep them working.
- Flatpak: needs raw netlink, `iw`, `dhcpcd` and sudo on the host; the sandbox gets in the way of exactly what rokast does.
