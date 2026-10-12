# Changelog

Each `## <version>` section becomes the GitHub release notes for that tag (see `playbooks/release.md`).

## 0.3.0

- **Packages:** an Arch/Manjaro AUR package (`rokast`, and `rokast-git` for main) and a `.deb` for Debian 12+ /
  Ubuntu 22.04+, attached to each release. Both install to `/usr/lib/rokast` with the patched `wpa_supplicant`
  prebuilt, so there is nothing to compile; `rokast` is on your PATH and in the app menu.
- **Update notice:** once a day the desktop app checks for a newer release and shows a banner and a tray menu line
  linking to it. `rokast version --check` does the same from the terminal. Nothing is downloaded; `CHECK_UPDATES=0`
  turns it off. Packaged installs are told to update with their package manager.
- rokast has its own icon now.
- `rokast doctor` no longer crashes when Wi-Fi or wpa_supplicant is not available; it reports it.

## 0.2.0

- `scripts/install`: installs for your user into `~/.local/share/rokast/versions/<version>/` with a `current` link,
  puts `rokast` on your PATH and in the app menu, and keeps the previous version for `scripts/install --rollback`.
  `--uninstall` removes everything except your settings.
- Settings now live in `~/.config/rokast/rokast.conf` (`rokast config` prints the path, `ROKAST_CONFIG` overrides it).
- The patched `wpa_supplicant` now lives in `~/.local/share/rokast/supplicant/`, outside the install folder, and is only
  rebuilt when its patch changes. `rokast doctor` reports a supplicant built from an older patch.
- CI runs the tests, shell checks and desktop app build on every push.
- `rokast selftest`: captures and encodes 5 seconds to a file and checks it with ffprobe, no TV needed.
  `rokast doctor --report` adds system info and the selftest, for the new hardware report issue template.
- **X11 now mirrors one monitor**, the primary one by default, instead of the whole X screen (with two monitors
  that was both side by side, squeezed onto the TV). `MONITOR=HDMI-1` picks another; `rokast monitors` lists them,
  and the desktop app has a Monitor dropdown when there are two or more.
- Desktop app: **Start with my computer, in the tray** (off by default) adds a login autostart entry that starts in
  the tray. The window size and the Details pane state are remembered.
- Fixed: VAAPI encoding (Intel/AMD) failed: ffmpeg's h264_vaapi needs `constrained_baseline` and a numeric level.

### Upgrading from 0.1.0

1. Extract the new tarball anywhere and run `scripts/install` from it. It needs the same build packages as
   `scripts/build-supplicant` did, because it rebuilds the patched `wpa_supplicant` in its new location.
2. Run `rokast doctor`.

Your `config/rokast.conf` is moved to `~/.config/rokast/rokast.conf` (the old file is renamed `rokast.conf.moved`).
The hostap source checkout from the old folder is reused, so nothing is downloaded again. The app menu entry and
`~/.local/bin/rokast` now point at the new install. After that, the old rokast folder is no longer used and can be deleted.

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
