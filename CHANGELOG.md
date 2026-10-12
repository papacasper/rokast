# Changelog

Each `## <version>` section becomes the GitHub release notes for that tag (see `playbooks/release.md`).

## Unreleased

Rename this heading to `## 0.2.0` at release time.

- `scripts/install`: installs for your user into `~/.local/share/rokast/versions/<version>/` with a `current` link,
  puts `rokast` on your PATH and in the app menu, and keeps the previous version for `scripts/install --rollback`.
  `--uninstall` removes everything except your settings.
- Settings now live in `~/.config/rokast/rokast.conf` (`rokast config` prints the path, `ROKAST_CONFIG` overrides it).
- The patched `wpa_supplicant` now lives in `~/.local/share/rokast/supplicant/`, outside the install folder, and is only
  rebuilt when its patch changes. `rokast doctor` reports a supplicant built from an older patch.
- CI runs the tests, shell checks and desktop app build on every push.
- `rokast selftest`: captures and encodes 5 seconds to a file and checks it with ffprobe, no TV needed.
  `rokast doctor --report` adds system info and the selftest, for the new hardware report issue template.
- Fixed: VAAPI encoding (Intel/AMD) failed: ffmpeg's h264_vaapi needs `constrained_baseline` and a numeric level.

### Upgrading from 0.1.0

Run `scripts/install` from the new release. Your `config/rokast.conf` is moved to `~/.config/rokast/rokast.conf`
(the old file is renamed `rokast.conf.moved`), and an existing `vendor/hostap-src` build tree is reused.

## 0.1.0

First release.

- Native Miracast (Wi-Fi Display) from Linux to Roku TVs, with nothing installed on the TV: 1080p30 H.264 + AAC audio,
  NVENC / VAAPI / libx264 picked automatically.
- Joins the TV's Wi-Fi Direct group with a patched `wpa_supplicant` (`scripts/build-supplicant`) while your normal
  Wi-Fi connection, routes and DNS stay untouched; everything is torn down on exit.
- Only your own TV can be selected: a Wi-Fi Direct peer must match a Roku on your LAN by name and MAC.
- X11 and Wayland capture (xdg-desktop-portal ScreenCast + PipeWire via GStreamer; screen choice remembered).
- The TV shows this computer's name (configurable with `NAME` or in the app).
- Desktop app (GTK 4, Rust): pick TV, quality, bitrate and audio, live progress, log pane.
- System tray icon coloured by state, with Start/Stop, Show/Hide and Quit; `rokast ui --tray` starts hidden.
- `rokast list | start | name | doctor | ui | version`.

Release tarball: source plus a prebuilt `ui/target/release/rokast-ui` (x86_64, GTK 4.6+, glibc 2.35+), so the desktop
app installs without Rust. You still build the patched `wpa_supplicant` with `scripts/build-supplicant`.
