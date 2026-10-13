# Plan: TV-only sound, picking the sound, extended display

Status: approved 2026-10-12 (all three recommendations). Built in Rust after the port (`docs/plan-rust.md`, v0.5.0).
Split on 2026-10-13 (user: "start now" on the recommendation): **v0.6.0 = sound (phases 12-13)**, **v0.7.0 = extended
display (phases 14-15)**, so the sound fix ships without waiting on the root/debugfs and Wayland work. The routing
itself already exists: `src/rokast-core/src/audio.rs` (`Router::route`/`restore`, `recover`, `listing`, the
`pactl subscribe` watcher; ported in R1 with its unit tests and `src/rokast-core/tests/audio_routing.rs` against
`rokast-fake-pactl`). What v0.6.0 adds is the wiring, the CLI, the app menu, live tests and docs. Same rules as the earlier plans (compatibility, tests, files under 500 lines, live check
after `p2p`/`wfd`/`capture`/`cli` changes, no AI attribution). Each task has a **Done when** check that must pass by
running it, not by eye. Tick tasks off here as they land.

| Phase | Release | What | Who |
|---|---|---|---|
| 12 | v0.6.0 | Sound on the TV only (no echo), restored on stop or crash | Claude, user confirms on the TV |
| 13 | v0.6.0 | Pick what goes to the TV: one output device, or one app; app menu; docs; release | Claude |
| 13b | v0.7.0 | Fill the TV: switch the laptop to the TV's size while mirroring (`FIT=match`) | Claude, user confirms on the TV |
| 14 | v0.7.0 | Extended display on X11 (a real second monitor the TV shows) | Claude, user confirms on the TV |
| 15 | v0.7.0 | Extended display on Wayland (GNOME/KDE virtual monitor), app controls, docs, release | Claude, user confirms on the TV |

## What the user asked for (2026-10-12)

- "Separate audio": chosen as **TV only** (while mirroring, sound plays on the TV and the laptop speakers go quiet)
  and **pick an output or app** (only that device's or that app's sound goes to the TV; the rest stays on the
  laptop). Today rokast records a copy of the default output, so both play, slightly out of sync.
- "An extended display if we can": the TV becomes a second screen next to the laptop's, instead of a copy of it.

## How: sound

All through `pactl`, which talks to PulseAudio and to PipeWire's pulse server alike (this laptop: PipeWire 1.6.8).

- **TV only.** At start: `pactl load-module module-null-sink sink_name=rokast sink_properties=device.description="rokast (TV)"`,
  remember the current default output, make `rokast` the default, and move every playing stream onto it
  (`pactl move-sink-input`). ffmpeg records `rokast.monitor` instead of the old default's monitor. The laptop plays
  nothing because no stream goes to its speakers. At stop: default output and streams back, module unloaded.
- **One output device.** Record that device's monitor (`AUDIO=device:<sink name>`). Useful with a spare or virtual
  output that the user routes things to.
- **One app.** Move only that app's streams to `rokast` (matched by `application.process.binary`, falling back to
  `application.name`), and keep watching with `pactl subscribe`. An app often opens a new stream for each video, so
  new streams from it are moved too. Everything else stays on the laptop.
- **Crash safety.** The state (module id, old default, moved streams) goes in
  `$XDG_RUNTIME_DIR/rokast/audio.json` before anything changes. Normal stop, SIGTERM and the next `rokast start` all
  restore from it, and `rokast doctor` reports leftovers and fixes them with `--fix`. If the laptop crashes, the
  sound server restarts clean anyway.
- **Settings.** `AUDIO=tv` (TV only), `both` (today's copy), `off`, `device:<sink>`, `app:<binary>`. `AUDIO=1`/`0`
  stay valid (see decision 1). `rokast audio [--json]` lists outputs and the apps playing now, for the app's menu
  and for scripts.

## How: extended display

**X11 (this laptop, XFCE).** Tested on 2026-10-12. Intel's driver has no virtual outputs, but the kernel lets root
make an unused video port act as connected with a chosen screen description (EDID):
`/sys/kernel/debug/dri/<gpu>/HDMI-A-2/edid_override` + `force` = `on`. X then sees a real `HDMI-2` with a
1920x1080 mode, and GTK (so xfwm4, panels and maximise) sees two monitors. rokast would:

1. pick a disconnected port on the GPU that drives the X screen (map the kernel name `HDMI-A-2` to the X name
   `HDMI-2`). Write an EDID made for the negotiated TV resolution (1080p or 720p), set `force` to `on` (via `sudo`,
   like the Wi-Fi setup already does);
2. `xrandr --output HDMI-2 --mode 1920x1080 --pos <laptop width>x0`. In the test, `--right-of` left it at 0,0, so
   set the position explicitly and check it with `xrandr --listmonitors`;
3. capture that monitor's area (the existing `MONITOR=` path);
4. at stop: output off, `force` back to `unspecified`, `edid_override` `reset`. Note `printf`, not `echo`: in the
   test, `echo` into these files gave "write error: Invalid argument". Crash safety through a state file, as for
   sound.

Limits:
- needs debugfs (blocked under kernel lockdown, e.g. some Secure Boot setups);
- needs a free port on an Intel or AMD GPU (i915, xe, amdgpu). NVIDIA's proprietary driver has the same debugfs files
  but ignores them: tested 2026-10-12 on this laptop's RTX 4060 (`HDMI-A-3` forced on, X's `HDMI-1-0` stayed
  disconnected). Hybrid laptops like this one are fine: the Intel GPU drives the desktop and its free ports are
  used, so the NVIDIA card neither helps nor gets in the way. Only NVIDIA-only machines get "basic".

Fallback: when this is unavailable, `xrandr --setmonitor` declares a region beside the laptop screen. ffmpeg can
capture it (tested), but GTK ignores monitors with no output, so windows don't open or maximise there; you drag
them over by hand. The app calls this "basic". XFCE may pop up its "display connected" dialog when the port
appears; Phase 14 checks for that and suppresses it if so.

**Wayland.** The screen-sharing portal that `rokast-core::wayland` already uses can make a virtual monitor instead of sharing
an existing one: source type `VIRTUAL` (4) instead of `MONITOR` (1). It's supported by GNOME (Mutter) and KDE
(KWin). The compositor creates the monitor and streams it; no root needed. The headless GNOME CI job can check
it.

**Settings.** `SCREEN=mirror` (default) or `extend`, plus `EXTEND_SIDE=right|left|above`. The app gets a
"Screen: Mirror / Extend" choice. The tray icon and window work the same way in both modes.

## Decisions (for the user)

1. **What `AUDIO=1` means from now on:** recommended **TV only** (fixes the echo for everyone who has the default).
   `AUDIO=both` keeps today's copy. The alternative is to leave `1` = both and make new installs and the app default
   to TV only.
2. **Extended display on X11 writes to a kernel debug file as root** (through the same `sudo` rokast already uses
   for Wi-Fi). It makes a spare video port look connected while extending, and puts it back at stop. Recommended
   **yes**, with the `--setmonitor` fallback when that's not possible.
3. **Default screen mode stays Mirror.** Extend is a choice in the app and in `SCREEN=`.

---

## Phase 12 (v0.6.0): sound on the TV only

- [x] **Routing** (`rokast-core::audio`): `parse_mode`, `Router::route` -> the pulse source to record, `restore`,
      `recover`, state in `$XDG_RUNTIME_DIR/rokast/audio.json`; unit tests + `tests/audio_routing.rs`. Done in R1.
- [x] **Wiring** (`src/rokast/src/start.rs`; before every push: `bash tests/check.sh`): `recover()` at start (leftover from a crash, said in the log); route when
      the TV starts playing (only when the negotiated audio is AAC and AUDIO != off), record the returned source
      instead of `capture::default_monitor()`, restore after ffmpeg stops, in every exit path (normal stop, SIGTERM,
      a failed join, the retry; a `Drop` guard so an early return can't skip it). A bad `AUDIO=` fails before joining.
      `selftest` uses the same source choice. Config default `AUDIO=tv` (`1` still means tv, decision 1).
      **Done when:** `cargo test --workspace` green, parity (`tests/parity/run.sh`, `cli.sh`) unchanged or its
      expected files updated on purpose (the `AUDIO=1` meaning is the one intended change), and
      `tests/test_audio_live.sh` (local, real sound server, no TV) passes: a `paplay` tone during a session started
      with a fake TV is in `rokast.monitor`, the old default sink has no inputs during it; after stop and after
      SIGTERM, default output, stream sinks and module list match the snapshot from before. Then the live check:
      TV plays sound, laptop silent (`pactl list sink-inputs` shows the player on `rokast`), after SIGTERM the
      player is back on the speakers. The user confirms on the TV.
      Done 2026-10-13: code, tests, parity green; `test_audio_live.sh` drives `audio_hold` (an example binary that
      routes like a playing session until SIGTERM) instead of a full fake-TV `start` (none exists), and passed 3
      runs in a row; found and fixed PipeWire dropping a move to a just-created sink. **Open: the TV check.**

## Phase 13 (v0.6.0): pick what goes to the TV, app, docs, release

- [x] **`rokast audio [--json]`** (outputs + apps playing now, from `audio::listing`), and `rokast doctor` reports
      leftover routing (`--fix` restores it).
      **Done when:** `tests/test_audio_live.sh` gains cases: two `paplay` streams with different
      `application.name`, `AUDIO=app:<one>`: only that one reaches `rokast.monitor`, the other stays on the
      speakers; a new stream from it started mid-session is moved within 1 s; `AUDIO=device:<sink>` records that
      sink's monitor and moves nothing.
- [x] **App:** "Sound" dropdown replacing the on/off switch: On the TV only / On the TV and the laptop / Not sent /
      One app (submenu from `rokast audio --json`) / One output. Old saved `audio: true/false` maps to tv/off. Rust
      unit tests for building the menu from that JSON and for the AUDIO value it sets. `test_ui_starts.sh` green.
- [x] **Docs:** README (Sound section, settings table `AUDIO` values), `docs/how-it-works.md` (the null sink),
      `config/rokast.conf.example`, CHANGELOG (the `AUDIO=1` change: the laptop goes quiet while mirroring;
      `AUDIO=both` for the old behaviour).
- [x] **TV check** (2026-10-13, Living Room): `tests/live_sound_tv.sh "Living Room" 6` -> ok: 6 chimes on `rokast`,
      heard on the TV with the laptop silent; restored after SIGTERM. (The TV had stopped advertising Wi-Fi Direct after
      3.7 days up; Home and ECP PowerOff/PowerOn did not help, a System restart did.)
- [x] **Release v0.6.0** by `playbooks/release.md`; the installed 0.5.0 updates itself (`rokast update`). Done 2026-10-13: run 38479760533 green, 6 assets, signature + sums OK; laptop updated 0.5.0 -> 0.6.0.

## Phase 13b (v0.7.0): fill the TV (`FIT=match`)

User, 2026-10-13: "Can we have the video size be the same size as the tv". The laptop is 1920x1200 (16:10), the TV
1920x1080, so capture letterboxes (black side bars, ~5% each). Tried by hand the same day: `xrandr --output eDP-1
--mode 1920x1080` before `rokast start` -> TV full screen, nothing cut or stretched; user: "Its the correct size now",
"build it". Zoom (crops 60 px top and bottom) and stretch (11% wide) were offered and not chosen.

- `FIT=bars` (default, today's letterbox) or `FIT=match`: once the TV plays (like the sound routing), switch the
  captured X11 output to an xrandr mode of the negotiated size, re-read its geometry, capture that; put the old mode
  (and rate) back when the stream ends, on SIGTERM, on the next start after a crash and with `rokast doctor --fix`
  (state in `$XDG_RUNTIME_DIR/rokast/fit.json`, written before switching). No such mode on that output, a
  `--setmonitor` monitor, or Wayland -> say so and keep bars (never fatal). Wayland would need each compositor's own
  display API; not planned.
- [x] **`src/rokast-core/src/fit.rs`:** parse `xrandr --query` (outputs, current mode and rate, modes with rates),
      pick the mode, `Fitter::apply/restore` + `Drop`, `recover`. Unit tests on this laptop's recorded listing.
- [x] **Wiring:** `start.rs` applies it in `on_play` for X11 and restores in `stream()`'s exit; a bad `FIT=` fails before
      joining; `doctor` checks `FIT=` and the leftover state (`--fix` puts it back). Parity outputs unchanged.
- [x] **App:** "Fill the TV" switch -> `FIT=match`/`bars`, remembered like the others.
- [x] **Docs:** README settings table and a short "Picture size" note, `config/rokast.conf.example`, CHANGELOG.
      **Done when:** `cargo test --workspace` green; `tests/test_fit_live.sh` (this laptop, no TV; it has to use the
      real panel, Xvfb has no modes to switch): `examples/fit_hold` switches to 1920x1080 and `xrandr --listmonitors`
      shows 1920x1080; SIGTERM -> the original mode and rate are back; SIGKILL -> `rokast doctor` FAILs, `--fix`
      restores. Live: `FIT=match rokast start` on Living Room -> `capture: ... 1920x1080`, TV full screen (user
      confirms), stop -> 1920x1200@165 again.
      **Result 2026-10-13:** unit tests green; `tests/test_fit_live.sh` ok (switch, SIGTERM restore, SIGKILL -> doctor
      FAIL -> `--fix` restore); live `FIT=match` on Living Room: `fit: eDP-1 switched to 1920x1080 for the TV` right
      after negotiation, streamed 25 s (the TV tolerates the mode switch between PLAY and the first frame), SIGTERM ->
      exit 0, 1920x1200@165 back, no fit.json left. Same picture the user confirmed by hand ("Its the correct size now").

## Phase 13c (v0.7.0): smooth motion (`FPS=60`)

User, 2026-10-13: "Can we have the refresh rate higher on the tv", then "Try higher hz". The TVs advertise HD at 30
fps at most (Living Room's CEA bitmap 0x0001d4bf: 1080p/720p at 24/25/30, 60 only at 480p). Tried anyway: negotiate
1080p30, send 1080p **60** fps from a 60 Hz laptop (`FIT=match`). nvenc refuses 1080p60 at level 4.0 ("Invalid
Level"); labelled level 4.2 the TV played it (`state=play`, position kept pace with the wall clock). User: "its
smoother, make it a setting, and make it work for the extended display".

- `FPS=tv` (default: the negotiated rate) or `FPS=60`: capture and encode at 60 fps whatever was negotiated, with
  the H.264 level raised to what the size needs at 60 (1080p: 4.2, 720p: 3.2). The laptop screen runs at 60 Hz while
  mirroring so frames are evenly spaced (165 Hz / 60 is uneven): `FIT=match` already does; with `FIT=bars` the same
  mode switches to its ~60 Hz rate (same restore paths as Phase 13b). The second screen is 60 Hz already.
- [x] **Core:** `capture::smooth(n, fps)` (pure, unit-tested: level per size), `Fitter` learns "same mode, 60 Hz".
- [x] **Wiring:** `start.rs` sends the smoothed mode to ffmpeg/gst for mirror, fill and extend; bad `FPS=` fails before
      joining; `doctor` checks it.
- [x] **App:** "Smooth motion (60 fps)" switch, remembered.
- [x] **Docs:** README settings table + a note under Picture size, config example, CHANGELOG.
      **Done when:** check.sh green; live on Living Room with `FPS=60` mirror (bars) and `FPS=60 SCREEN=extend`:
      ffmpeg runs at `-framerate 60`, TV `state=play`, position keeps pace with the wall clock over 20 s, laptop back
      at 1920x1200@165 after stop. The user confirms on the TV.

Results (2026-10-13, Living Room): `FPS=60` mirror: laptop switched to 1920x1200@60.00, ffmpeg `-framerate 60 -level
4.2`, TV `state=play`, 20.05 s in 20.03 s wall; `FPS=60 SCREEN=extend`: TV screen HDMI-1 1920x1080@60, 19.8 s in
20.0 s; both back to 1920x1200@165, one monitor, wlo1 only. **Bug found:** start measured the screen before undoing a
crashed session's leftover mode, so after a killed run it captured 1920x1080 of a 1920x1200 screen (cropped picture,
TV dropped). Fixed: the recovers run first; `tests/test_start_order.sh` (in check.sh) keeps that order.
User confirmation on the TV: pending.

## Phase 14 (v0.7.0): extended display on X11

- [x] **`src/rokast-core/src/extend.rs`:** EDID builder (pure, unit-tested against `edid-decode` when present), free-port
      finder (debugfs + `xrandr` name mapping, pure parts unit-tested on recorded listings), `start(res, side)` ->
      state, `stop(state)`, fallback `--setmonitor`. Crash safety via `$XDG_RUNTIME_DIR/rokast/extend.json`.
- [x] **`start.rs`:** `SCREEN=extend` creates the monitor before capture and captures it, then removes it in every
      exit path. `rokast doctor` reports whether extend is "full", "basic" or unavailable, and why.
      **Done when:** `tests/test_extend_live.sh` (this laptop, no TV): after start, `xrandr --listmonitors` has 2
      monitors, the new one at the expected position. GDK lists it, and an `x11grab` of its area works. After stop,
      `xrandr --listmonitors`, `force` and `edid_override` match the snapshot taken before. Then the live check with
      `SCREEN=extend`: a window dragged right shows on the TV and only there; SIGTERM -> back to one monitor, exit
      0, only `wlo1`. The user confirms on the TV.

Results (2026-10-13): `tests/test_extend_live.sh` ok; live on Living Room "extend: the TV is screen HDMI-1
1920x1080+1920+0 (full)", streamed, back to one monitor and wlo1; again with `FPS=60` (Phase 13c). The user went
ahead with the release ("Okay lets do this") without reporting the window-drag check on the TV.

## Phase 15 (v0.7.0): the app, release

Decided 2026-10-13: v0.7.0 ships X11-only extend; the Wayland virtual monitor moves to Phase 16 (v0.8.0).

- [x] **App:** done in Phase 14 ("TV as a second screen" switch, remembered; Fill and Smooth motion beside it).
- [x] **Docs:** README (Picture size, Smooth motion, Second screen, settings table), `docs/how-it-works.md` (picture
      options), CHANGELOG.
- [ ] **Release** by `playbooks/release.md`.

## Phase 16 (v0.8.0): Wayland second screen

- [ ] **Wayland virtual monitor:** `rokast-core::wayland` asks for source type `VIRTUAL` when `SCREEN=extend` (and when the
      portal's `AvailableSourceTypes` has it). If not, it falls back to mirror with a clear message.
      **Done when:** the `wayland` CI job's GNOME run gains an extend case: the selftest encodes a frame from the
      virtual monitor, and `gdbus` shows Mutter with 2 monitors during it.
- [ ] **App + docs:** the switch says when the portal has no virtual monitor; README and how-it-works on Wayland.
- [ ] **Release** v0.8.0 by `playbooks/release.md`.

## Not planned

Delaying the laptop's sound to stay in sync with the TV (never exact over Wi-Fi), more than one extra screen, the
TV as a screen while the laptop lid is closed (works already by the laptop's own settings), NVIDIA-only laptops
getting a "full" extended display on X11 (no driver switch for it; they get "basic").
