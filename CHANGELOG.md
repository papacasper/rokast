# Changelog

Each `## <version>` section becomes the GitHub release notes for that tag (see `playbooks/release.md`).

## Unreleased

- **Sound in step with the picture:** `AUDIO_DELAY` (milliseconds, as in mpv) shifts the TV's sound. Negative plays
  it earlier, for TVs that show the picture before the sound; `rokast doctor` checks the value.

## 0.8.1

- **Fix: joining the TV failed on some tries.** When the TV took two Wi-Fi scans to open its pairing window, the join
  completed a fraction of a second after rokast had given up, on every attempt. rokast now waits 25 s per attempt and
  never cuts off a join that is already connecting.

## 0.8.0

- **Second screen on Wayland (GNOME, KDE):** `SCREEN=extend` (the app's **TV as a second screen** switch) now works
  on GNOME and KDE Plasma Wayland too: the desktop adds a virtual screen of the TV's size through the screen-sharing
  portal and rokast streams it. Other Wayland desktops still mirror (and say so); `rokast doctor` tells which.
  Untested on real GNOME/KDE hardware so far (passes CI on headless GNOME).

## 0.7.0

- **Fill the TV:** `FIT=match` (the app's **Fill the TV** switch) makes the laptop screen take the TV's size while
  mirroring, so a 16:10 laptop fills a 16:9 TV with no black bars and nothing cut off. It switches back when you
  stop, and after a crash on the next start or with `rokast doctor --fix`. X11 only; `FIT=bars` (default) is unchanged.
- **TV as a second screen:** `SCREEN=extend` (the app's **TV as a second screen** switch) makes the TV an extra
  screen beside the laptop (`EXTEND_SIDE=right|left|above|below`) instead of a copy. On X11 a free Intel/AMD video
  port is made to look connected with a screen description of the TV's size, so windows open and maximise on it;
  `rokast doctor` says whether that works here ("full") or only "basic" (drag windows over). Removed when you stop,
  after a crash on the next start or with `rokast doctor --fix`.
- **Smooth motion:** `FPS=60` (the app's **Smooth motion** switch) sends 60 frames a second instead of the 30 the TV
  asks for at HD, in mirror, Fill the TV and second-screen modes. On X11 the laptop screen runs at 60 Hz while
  mirroring so frames are evenly spaced. `FPS=tv` (default) is unchanged.

## 0.6.0

- **Sound plays only on the TV:** while mirroring, the laptop goes quiet and the TV gets no echo (`AUDIO=tv`, the
  new default; `AUDIO=1` now means this too). The sound goes back when you stop, and after a crash on the next
  start or with `rokast doctor --fix`. `AUDIO=both` keeps the old behaviour (TV and laptop).
- **Pick what goes to the TV:** `AUDIO=app:<name>` sends one app only (its new streams too), `AUDIO=device:<output>`
  one output. `rokast audio` lists both; the app's **Sound** menu replaces the on/off switch.

## 0.5.0

- **One Rust program, no Python:** `rokast` is now a single Rust binary (prebuilt in releases and packages; a
  git clone builds it with cargo). Python is no longer needed, and the packages no longer pull it in. Commands,
  output and config are unchanged (checked against answers recorded from the Python version).

## 0.4.0

- **Desktop app opens again:** the 0.3.0 app closed straight away at start (it set its icon before GTK was ready).
  CI now starts the app on a headless screen, so this can't ship again.
- **Self-update:** `rokast update` installs the latest release: it downloads it, checks its signature and checksum,
  installs it beside the current one and checks it runs, rolling back by itself if not (`rokast update --rollback`
  to go back by hand). The app's update banner has an **Update** button and then **Restart rokast**; the tray line
  does the same. It refuses while you are mirroring. `.deb`/`.rpm` installs update through your package manager
  (it asks for your password; without a password prompt it prints the command to run). AUR and git installs are
  told how to update. `AUTO_UPDATE=1` in `rokast.conf` installs updates when the app starts (off by default).
- **Signed releases:** each release's `SHA256SUMS` is signed (`SHA256SUMS.sig`); `rokast update` only installs a
  release signed by the key in `config/release-signers`. Check one by hand with
  `ssh-keygen -Y verify -f config/release-signers -I rokast-release -n rokast-release -s SHA256SUMS.sig < SHA256SUMS`.
- `scripts/install` builds the supplicant before switching to the new version, so a failed build changes nothing.
- **Upgrading:** from 0.3.0 or older, update by hand once (download the release and run `scripts/install`, or install
  the new package); after that, `rokast update` does it.
- **Join reliability:** fixed the first connection after the TV sat idle often failing (stuck "Connecting", then
  "could not join"). The TV hides its group's name in its beacons and does not always answer a general search;
  rokast now asks for the group by name. If a start still fails before the picture appears, it starts over once
  by itself ("starting over once more").
- **Fedora package:** `rokast-X.Y.Z-1.x86_64.rpm` for Fedora 43+, attached to each release (`sudo dnf install
  ./rokast-*.rpm`). With Fedora's own ffmpeg (no libx264) rokast now falls back to the libopenh264 encoder.
- **Evidence for join problems:** a failed join attempt's details are kept in `~/.cache/rokast/join-failures/`
  (newest 5), and `rokast doctor --report` includes the latest, so a bug report can show what went wrong.
- **Wayland on GNOME tested:** a new CI job runs the Wayland capture through GNOME's real screen-sharing dialog
  (headless GNOME Shell 49, Fedora 43) and it works. KDE's dialog is exercised too, but its screen capture needs a GPU
  the CI machines lack, so KDE is still marked "needs a report".

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
