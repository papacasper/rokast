# Plan: rokast in Rust

Status: approved 2026-10-12 (all three recommendations). R1 done; R2 and R3 done on the `rust-port` branch (merged after v0.4.0 ships, so that release stays Python-only); R4 next. The user chose to port first (2026-10-12: "go with a"),
so `docs/plan-audio-extend.md` (sound, extended display) is on hold and gets built in Rust after phase R6. Same
rules as the earlier plans (compatibility, tests, files under 500 lines, live check after `p2p`/`wfd`/`capture`/`cli`
changes, no AI attribution). Each task has a **Done when** check that must pass by running it, not by eye. Tick tasks
off here as they land.

| Phase | What | Release | Who |
|---|---|---|---|
| R0 | Ship today's main (fixes the 0.3.0 app crash) before the port starts | v0.4.0 | user login check, then Claude |
| R1 | Cargo workspace, `rokast-core` with the pure parts, parity harness against Python | (none) | Claude |
| R2 | Rust `rokast` binary: every command except `start` and `update` | (none) | Claude |
| R3 | `start` in Rust: Wi-Fi Direct, RTSP/WFD, capture, X11 | (none) | Claude, user watches the TV |
| R4 | Wayland screen sharing in Rust | (none) | Claude |
| R5 | `update`/`rollback` in Rust, upgrade from the Python versions | (none) | Claude |
| R6 | Python removed, packaging, docs, soak, release | v0.5.0 | Claude, user confirms on the TV |

## Why, and what changes for users

The user asked for "most if not all of this in Rust" (GitHub: 54% Python). Today the window/tray app is Rust and
everything it drives (`rokast list/start/doctor/update...`) is ~2,200 lines of Python (`src/miracast/`, standard
library only) with ~1,360 lines of Python tests.

After the port:
- one compiled `rokast` program next to `rokast-ui`;
- no Python 3.10+ needed, and no PyGObject on Wayland;
- the same commands, settings file, status lines and exit codes, so the app, scripts and docs keep working.

What stays as it is:
- ffmpeg, GStreamer, `iw`, `dhcpcd`, `pactl`, `ssh-keygen` and `sudo` stay separate programs that rokast runs;
- the patched `wpa_supplicant` (C) is unchanged;
- shell stays for `scripts/install`, packaging and the shell tests.

Expected end state on GitHub: no Python, Rust around 75-85%, shell the rest.

## How

**Layout.** A Cargo workspace at the repo root:
- `src/rokast-core/` (library: everything that isn't a command line);
- `src/rokast/` (the `rokast` binary: argument parsing, status lines);
- `ui/` (unchanged path, so the packaging scripts don't move).

`ui/` is **not** a workspace member: it stays its own Cargo project with its own `Cargo.lock` and `ui/target/`.
`ui/target/release/rokast-ui` is baked into installs, menu entries, `scripts/install`/`install-ui`, the release and
packaging scripts and the `test_pkg_*`/`test_install` tests; as a member it would build to the root `target/` instead.
(The root `Cargo.toml` lists it under `exclude`.) Code formatting: `src/rustfmt.toml` (width 120, like the Python);
`ui/` keeps rustfmt's defaults.

During the port, `src/miracast/` stays as the reference.

**Moving over one command at a time.** The Rust `rokast` runs a command itself once it is ported, and hands the
others to the Python `cli.py` (exec, same arguments). The only exception is `start`, which needs every module, so
it moves in one step (R3) behind the same check as always: the live check, then the soak. `scripts/rokast` points at
the Rust binary from R2 on. The user can force Python for a command with `ROKAST_PY=1` until R6, as an escape hatch if
something regresses.

**Proof of sameness, not "looks the same".** For each module, a parity test runs Python and Rust on the same inputs
and compares the output byte for byte. It's a small harness, `tests/parity/`, that calls both through tiny entry
points. It covers:
- WFD message parsing/formatting over recorded TV traffic;
- the ffmpeg/GStreamer command lines for a matrix of formats, encoders and capture modes;
- config parsing and the version/update notice logic;
- `xrandr` monitor parsing;
- the sound routing state machine against `tests/fake_pactl.py`.

On top of that, the shell tests already drive `rokast` as a black box (`test_self_update.sh`, `test_pkg_*`,
`test_install.sh`, `test_monitor_capture.sh`, `test_ui_starts.sh`, `wayland_desktop.sh`) and keep passing unchanged.

**Crates (few, common, pure Rust where possible):**

| Need | Crate |
|---|---|
| JSON | `serde`, `serde_json` (the app already uses them) |
| D-Bus for the Wayland portal | `zbus` (blocking API) |
| Update checks and downloads | `ureq` with `rustls` |
| Checksums | `sha2` |
| Release tarballs | `flate2` + `tar` |
| File locks | `libc` (already in the app) |

Network code (RTSP, ECP to the TV) uses plain `std::net` and threads, not an async runtime. It is one TV and a
handful of connections. Signatures keep using `ssh-keygen -Y verify`, so trust doesn't change.

**The app keeps running `rokast` as a separate process** and reading its status lines. This keeps `sudo` prompts and a
crashing capture out of the window's process. It shares only types (settings, status parsing) with `rokast-core`.

## Decisions (for the user)

1. **Ship v0.4.0 first (R0):** recommended **yes**. Released 0.3.0's app crashes at start, and the port will take a
   while. Needs your logout/login autostart check first, as planned.
2. **No releases mid-port; the first Rust release is v0.5.0** with the self-updater, followed by sound and extended
   display (v0.6.0). Recommended **yes**. The alternative is a hybrid Rust+Python release after R2, which saves nothing
   for users because Python is still needed.
3. **The app keeps talking to `rokast` through a separate process**, not by linking the core. Recommended **yes**
   (see above).

Not a decision: prebuilt binaries are x86_64 only, as the app already is. aarch64 can come later from CI if asked.

---

## Phase R0: ship today's main as v0.4.0

- [ ] The user's logout/login autostart check, then the release by `playbooks/release.md` (the self-updater phases
      8-10 ship in it, so v0.4.0 is the first version that can update itself).
      **Done when:** the release workflow is green, `rokast-ui` from the release starts (`tests/test_ui_starts.sh`
      against the release binary), and `rokast update --check` from v0.3.0 sees 0.4.0.

## Phase R1: workspace, core, parity harness

- [x] Workspace (`Cargo.toml` at the root, `src/rokast-core`, `src/rokast`; `ui` excluded, see Layout). CI's `ui` job
      became `rust`: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, parity, coverage, and
      the app build/start as before. `scripts/release` bumps both `Cargo.toml`s and runs the workspace tests.
- [x] Port the pure parts with their unit tests: `paths`, `update` (versions, cache, kind, lock), `wfd` (Negotiated,
      format parsing, choice, messages), `capture` (monitors, `build_cmd`), `joinlog`, `selftest` (checks,
      summary), `audio` (parse/route/restore logic). Also `conf`, `wayland` (`gst_cmd`) and the pure `p2p` helpers.
      Sound routing runs against `tests/fake_pactl.py` (`src/rokast-core/tests/audio_routing.rs`).
- [x] `tests/parity/`: a Python side (`python3 -I` against `src/miracast`) and a Rust side (`rokast-core`'s
      `parity` example) print JSON for the same input set (`cases.json`, written by `make_cases.py`: 230 cases);
      `compare.py` diffs them. Errors shown to people must match word for word.
      **Done when:** `cargo test --workspace` and `tests/parity/run.sh` are green in CI. Each ported Python unit test
      has a Rust counterpart (`tests/parity/coverage.sh` maps every `test_*` name and fails on a missing one).

## Phase R2: Rust `rokast` for everything except start and update

- [x] `rokast` binary: `list [--json]`, `doctor [--report]`, `monitors [--json]`, `selftest`, `version [--check
      [--json]]`, `name`, `config`, `ui`; `start` and `update` are exec'd to `cli.py`, and so are `doctor`/`selftest`
      when capture is Wayland (the portal comes in R4). `scripts/rokast` runs `target/release/rokast` when it is built
      (`scripts/install` builds it with cargo, like the app), else Python; in a clone whose Rust sources are newer than
      the binary it says so and uses Python, so a stale build never hides a change. `--json` output is Python's
      `json.dumps` byte for byte (`pyfmt::dumps`). `audio` is not here: Python has no such command yet, it comes with
      sound in v0.6.0.
      **Done when:** the shell tests are green with the Rust binary, and `tests/parity/cli.sh` matches Python's
      output for each ported command on this laptop (`list --json` against the real TV, `doctor`, `monitors`,
      `selftest` file checks). `test_ui_starts.sh` is green.
      Result 2026-10-12: `cli.sh --live` 22/22 (three TVs, doctor, doctor --report, selftest); CI runs the 17 that
      need no hardware. `test_install`, `test_self_update`, `test_monitor_capture`, `test_ui_starts` green locally
      with the binary built. `src/rokast/tests/cli.rs` ports the version-check CLI tests (fake GitHub, real fetch).

## Phase R3: start in Rust

- [x] `p2p` (wpa_supplicant control, own-group start, peer find, DHCP, teardown), `wfd::Source` (RTSP server, M1-M7,
      keepalive, the 20 s RTSP wait), capture pipeline start/stop, the join retry, join failure logs, SIGTERM
      teardown, the session lock.
      **Done when:** the live check (start in the background -> `status: streaming`, `active-app` RokuCast,
      `kill -TERM` -> exit 0, only `wlo1` in `iw dev`) passes 3 times in a row. `tests/live_join_soak.sh 10 15` passes
      (every run streams, every join on attempt 1, the same bar as the Python version). The user confirms picture
      and sound on the TV.
      Result 2026-10-12: `link.rs` (join, DHCP, teardown), `rtsp.rs` (the RTSP source), `session.rs` (join retry),
      `stop.rs` (SIGTERM/SIGINT), `src/rokast/src/start.rs`; `update` and Wayland `start` still go to Python. The
      live check passed 3 of 3 through `scripts/rokast` (streaming in 19-24 s, every join on attempt 1, 1920x1080@30
      AAC, exit 0, only `wlo1` left). The soak was stopped by the user after run 1 (joined 1/4, streamed,
      clean teardown): "stop the soak we now know it works". The 11 R3 Python tests have Rust counterparts.

## Phase R4: Wayland in Rust

- [ ] Portal ScreenCast over `zbus` (session, select sources, start, OpenPipeWireRemote fd), GStreamer feeder.
      **Done when:** the `wayland` CI job (headless GNOME, `tests/portal_click.py` approving the dialog) passes with
      the Rust binary and `ROKAST_NO_PY=1` (the launcher and the binary refuse to fall back to Python).
      Changed from "no PyGObject installed in the job": the dialog clicker itself is PyGObject (AT-SPI), so the job
      keeps python3-gobject for the test harness only; ROKAST_NO_PY proves rokast never touches Python.

## Phase R5: update and rollback in Rust

- [ ] `rokast update [--check|--yes|--auto]`, `rokast rollback`: same messages, checks and refusals as
      `selfupdate.py`.
- [ ] **Upgrade across the switch:** a Python v0.4.0 install updates to a Rust release. The old Python updater runs
      the new tarball's `scripts/install`, which installs the prebuilt `rokast`.
      **Done when:** `tests/test_self_update.sh` and `tests/test_pkg_update.sh` pass with the Rust binary, and a new
      case passes: install real v0.4.0, update to a locally built Rust release, `rokast version` reports it, rollback
      returns to 0.4.0.

## Phase R6: Python out, release v0.5.0

- [ ] Delete `src/miracast/`, the Python unit tests that have Rust counterparts, the Python parity side and
      `ROKAST_PY`. CI drops the Python jobs.
- [ ] Packaging: .deb/.rpm/AUR without `python3` and `python3-gi`; AUR builds both binaries with `cargo`.
      **Done when:** `rg -il python packaging scripts .github` finds nothing. `tests/test_pkg_deb.sh`, `test_pkg_rpm.sh`
      and `test_pkg_arch.sh` pass in CI in images without Python. `gh api repos/papacasper/rokast/languages`
      shows no Python after the push.
- [ ] Docs: README (requirements, building from source), `docs/how-it-works.md`, CHANGELOG ("rokast is now one Rust
      program; Python no longer needed").
- [ ] Release v0.5.0 by `playbooks/release.md`, then the live check from the installed release.

## After this

`docs/plan-audio-extend.md` phases 12-15, written in Rust in `rokast-core` (the sound routing arrives already ported
in R1). Then the AUR push, which still needs the user's approval.

## Not planned

Rewriting the shell scripts or shell tests in Rust, linking ffmpeg/GStreamer as libraries instead of running them,
replacing `wpa_supplicant`, Windows/macOS.
