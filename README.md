<p align="center">
  <img src="assets/logo.svg" alt="Rokast — Linux to Roku screen mirroring" width="700">
</p>

# rokast

[![ci](https://github.com/papacasper/rokast/actions/workflows/ci.yml/badge.svg)](https://github.com/papacasper/rokast/actions/workflows/ci.yml)

Mirror a Linux desktop to a Roku TV over **native Miracast** (Wi-Fi Display), the same way Android
"Smart View"/"Cast" and Windows "Connect" (Win+K) do it. Nothing is installed on the TV and no
app or account is needed. The video goes from the laptop to the TV directly over Wi-Fi Direct.

- 1080p30 H.264 video + AAC audio, hardware encoding (NVENC / VAAPI) with software fallback
- X11 and Wayland (GNOME, KDE, sway, Hyprland... through the standard screen-sharing portal)
- ~20 s from `rokast start` to picture, Ctrl+C to stop and fully clean up
- Your normal Wi-Fi connection stays up the whole time (routes and DNS untouched)
- One Rust program plus `ffmpeg`, `iw`, `dhcpcd` and a patched `wpa_supplicant` (and GStreamer on Wayland); no Python

```
$ scripts/rokast start "Living Room"
target: Living Room (192.168.1.50, p2p 4e:xx:xx:xx:xx:xx)
joining the TV's Wi-Fi Direct group...
wfd: listening on p2p-wlo1-mir:7236 for the sink
wfd: sink 172.29.243.225 connected
link up: p2p-wlo1-mir 172.29.243.238, waiting for the TV to connect
wfd: negotiated 1920x1080@30 high L4.0 AAC
encoder: h264_nvenc
```

> **Why does this exist?** The standard Linux way to join a Wi-Fi Direct group
> (`wpa_supplicant`'s `p2p_connect ... join`, which NetworkManager-based tools use) gets a link to a
> Roku, but the Roku never starts the stream, at least on the hardware tested here. [docs/how-it-works.md](docs/how-it-works.md) explains what the Roku actually
> requires and why a small patch to `wpa_supplicant` is needed.

## Status / compatibility

`rokast selftest` runs the real capture + encode pipeline into a file for 5 seconds and checks it with ffprobe, without
a TV, so capture and encoding can be tested on any machine. `rokast doctor --report` adds it to a full hardware report.

| Path | Result | Tested on |
|---|---|---|
| Full mirror to a Roku | works | Roku TV G512X; Manjaro, XFCE on X11, Intel AX211 Wi-Fi (iwlwifi) |
| X11 capture + NVENC | works (selftest + live) | same laptop, NVIDIA RTX 4060 Mobile |
| X11 capture + VAAPI (Intel) | works (selftest) | same laptop, Intel Raptor Lake UHD, intel-media-driver |
| X11 capture + libx264 | works (selftest) | same laptop |
| Wayland, wlroots portal | works (live) | sway 1.12 + xdg-desktop-portal-wlr (headless session, same laptop) |
| Desktop app without a tray host | works | same laptop, app on a D-Bus session with no StatusNotifierWatcher |
| Wayland, GNOME portal | works (CI) | GNOME Shell 49 headless + xdg-desktop-portal-gnome, Fedora 43 (`wayland` workflow: selftest through the real share dialog) |
| Wayland, KDE portal | needs a report | Plasma 6 share dialog opens and is answered in CI, but KWin screencasts only with OpenGL compositing, which needs a GPU the CI runners lack; please [report](https://github.com/papacasper/rokast/issues/new/choose) how it goes |
| VAAPI (AMD) | untested | |
| Full mirror on another Wi-Fi chip | untested | |

It should work on any Roku device that accepts screen mirroring from Android phones, and on any
Linux machine whose Wi-Fi chip supports P2P-client mode (`iw list` shows `P2P-client` under
"Supported interface modes"; most Intel AX2xx/AX1xx/9xxx cards do). Reports for other hardware
are welcome: open a [hardware report](https://github.com/papacasper/rokast/issues/new?template=hardware-report.md)
with the output of `rokast doctor --report`.

Limitations:
- Needs **NetworkManager** with Wi-Fi P2P available (`sudo ls /run/wpa_supplicant` shows `p2p-dev-<wifi>`).
- Uses `sudo` for the network setup (interface creation, wpa_supplicant, dhcpcd).
- VAAPI needs your GPU's VA-API driver: `intel-media-driver` (Intel Broadwell and newer; the old `libva-intel-driver`
  does not support recent chips) or Mesa's VA-API driver (AMD). `ENCODER=auto` only picks VAAPI if a test encode
  succeeds, otherwise it falls back to libx264.
- Non-Roku Miracast sinks (Samsung/LG TVs, dongles) may work but are untested.

## Install

**Packages** (from v0.3.0; they include the patched `wpa_supplicant` and the desktop app; skip the steps below):

- Arch / Manjaro: from the AUR, `yay -S rokast` (or `rokast-git` for main).
- Debian 12+ / Ubuntu 22.04+: download `rokast_X.Y.Z_amd64.deb` from the
  [latest release](https://github.com/papacasper/rokast/releases/latest), then `sudo apt install ./rokast_*_amd64.deb`.
- Fedora 43+ (from v0.4.0): download `rokast-X.Y.Z-1.x86_64.rpm` from the latest release, then
  `sudo dnf install ./rokast-*.x86_64.rpm`. Fedora's own `ffmpeg-free` has no libx264, so rokast encodes with
  libopenh264 unless you have a GPU encoder; RPM Fusion's `ffmpeg` gives a sharper picture (`rokast doctor` says
  which encoder it uses).

Then run `rokast doctor`. Update with your package manager (or a newer package).

**From source / release tarball:**

1. Dependencies:

   ```bash
   # Arch / Manjaro
   sudo pacman -S --needed git base-devel ffmpeg iw dhcpcd wpa_supplicant libnl openssl
   # Debian / Ubuntu
   sudo apt install git build-essential pkg-config ffmpeg iw dhcpcd wpasupplicant libnl-3-dev libnl-genl-3-dev libssl-dev
   # (older releases: dhcpcd5; newer: dhcpcd-base)
   # Fedora
   sudo dnf install git gcc make pkgconf ffmpeg iw dhcpcd wpa_supplicant libnl3-devel openssl-devel
   # Wayland sessions additionally need GStreamer with PipeWire:
   #   Arch/Manjaro:  sudo pacman -S --needed gstreamer gst-plugins-base gst-plugin-pipewire
   #   Debian/Ubuntu: sudo apt install gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-pipewire
   #   Fedora:        sudo dnf install gstreamer1 gstreamer1-plugins-base pipewire-gstreamer
   # plus your desktop's portal backend (usually already installed: xdg-desktop-portal-gnome / -kde / -wlr / -hyprland)
   # A git clone also needs Rust (cargo, from https://rustup.rs or your distro) and the GTK 4 dev files for the app
   # (libgtk-4-dev / gtk4-devel / gtk4); a release tarball has both programs prebuilt.
   ```

2. Get rokast, either the latest [release](https://github.com/papacasper/rokast/releases) tarball
   (`tar xzf rokast-*-x86_64-linux.tar.gz && cd rokast-*`; `rokast` and the desktop app are prebuilt, so no Rust needed)
   or a git clone as below. Then install it for your user. This puts `rokast` on your PATH (`~/.local/bin`), adds the
   desktop app to the app menu, and builds the patched `wpa_supplicant` (it is only used by this tool, in its own
   control directory; your system `wpa_supplicant` is not replaced):

   ```bash
   git clone https://github.com/papacasper/rokast && cd rokast
   scripts/install
   ```

   Installed files live in `~/.local/share/rokast/` (one folder per version), settings in `~/.config/rokast/`.
   To upgrade, run `rokast update` (from v0.5.0 on: downloads the latest release, checks its signature, installs
   it; see [Updating](#updating)), or run `scripts/install` from the newer release yourself. Your settings are kept,
   and the supplicant is only rebuilt when its patch changed. `scripts/install --rollback` goes back to the previous version,
   `scripts/install --uninstall` removes everything except your settings. Upgrading from v0.1.0 moves your old
   `config/rokast.conf` to `~/.config/rokast/rokast.conf` automatically.

3. Configure (optional):

   ```bash
   mkdir -p ~/.config/rokast && cp config/rokast.conf.example ~/.config/rokast/rokast.conf   # set TARGET to your TV's name
   ```

4. On the TV: **Settings > System > Screen mirroring > Screen mirroring mode = Prompt** (or Always allow).

5. Check everything:

   ```bash
   rokast doctor
   ```

   (Without installing, everything also runs straight from the folder: `scripts/build-supplicant` once, then
   `scripts/rokast ...`.)

## Desktop app

A small GTK 4 window (written in Rust) for people who'd rather click: pick the TV, quality,
bitrate and where the sound goes, then press **Start mirroring**. It shows live progress (finding the TV,
connecting, waiting for the TV, mirroring), lets you set the **Name on TV**, has a Details pane with the full log, and cleans the
connection up when you press Stop or quit. It drives the same `rokast` command
underneath, so it behaves exactly like the CLI.

It also puts an icon in the **system tray**: grey when idle, amber while connecting, green while
mirroring, red on an error. Left-click shows or hides the window; right-click has Start/Stop mirroring,
Show/Hide window and Quit. Closing the window just hides it to the tray (mirroring keeps going), and
**Quit** stops mirroring and exits. `rokast ui --tray` starts straight into the tray, handy for autostart.
The tray works on desktops with StatusNotifierItem support (KDE, XFCE, Cinnamon, MATE, LXQt, Budgie,
waybar/sway/Hyprland bars). GNOME needs the *AppIndicator and KStatusNotifierItem Support* extension.
Without a tray, closing the window stops mirroring and quits.

Turn on **Start with my computer, in the tray** to have it start as a tray icon (no window) when you log in;
that writes `~/.config/autostart/io.github.papacasper.rokast.desktop`, and turning it off removes it. With two or
more monitors on X11, a **Monitor** dropdown picks which one to mirror (on Wayland the screen-sharing dialog asks).

```bash
# needs Rust (cargo) and GTK 4 development files:
#   Arch/Manjaro: sudo pacman -S --needed rust gtk4    Debian/Ubuntu: sudo apt install cargo libgtk-4-dev    Fedora: sudo dnf install cargo gtk4-devel
scripts/install        # (step 2) builds it, or uses the release tarball's prebuilt binary, and adds "rokast" to the app menu
rokast ui              # or launch from the menu; `rokast ui --tray` to start in the tray
```

Without passwordless sudo, it asks for your password in a dialog (`zenity` or `kdialog`).
Your last choices, the window size and whether Details is open are remembered in `~/.config/rokast/ui.json`.

## Usage

```bash
scripts/rokast list                  # Rokus on your LAN (name, IP, model); --json for scripts
scripts/rokast start "Living Room"   # or just `scripts/rokast start` with TARGET set in config
# Ctrl+C to stop
scripts/rokast selftest              # capture + encode 5 s to a file and check it (no TV needed); --seconds N, --out FILE
scripts/rokast doctor --report       # all checks + system info + selftest, for a hardware report
scripts/rokast monitors              # X11 monitors, for MONITOR= (--json for scripts)
scripts/rokast update                # install the latest release (see Updating); --check, --rollback
```

On first use the TV may ask whether to allow the device. It shows this computer's name (the pretty
hostname, else the hostname; set `NAME` or use the desktop app's **Name on TV** field to change it).
Choose **Always allow** so it does not ask again. `rokast name` prints the name that will be used.

On **Wayland** the desktop's screen-sharing dialog asks which screen to mirror on the first run.
The choice is remembered (portal restore token in `~/.config/rokast/portal-token`), so later runs
don't ask again on desktops that support it.

The laptop must be on the same LAN as the TV **once per run** so the tool can verify which TV is
yours (see Safety). The mirror itself runs over Wi-Fi Direct.

### Configuration (`~/.config/rokast/rokast.conf`)

`rokast config` prints the file in use; `ROKAST_CONFIG=/path/to/file` uses another one.
Environment variables with the same names override the file.

| Key | Default | Meaning |
|---|---|---|
| `TARGET` | | TV name exactly as `rokast list` prints it |
| `NAME` | this computer's name | Name the TV shows for this computer (max 32 bytes) |
| `RES` | `1080p30,720p60,720p30` | Preference order; the best mode the TV supports is used |
| `BITRATE` | `8M` | Video bitrate |
| `ENCODER` | `auto` | `auto`, `h264_nvenc`, `h264_vaapi`, `libx264` or `libopenh264` (Fedora's `ffmpeg-free`) |
| `AUDIO` | `tv` | Where the sound goes (see [Sound](#sound)): `tv` (only the TV; `1` means the same), `both` (TV and this computer), `off`, `app:<name>` (one app only), `device:<output>` (what plays on that output) |
| `CAPTURE` | `auto` | `auto` (Wayland session → `wayland`, else `x11`), `x11` or `wayland` |
| `MONITOR` | primary monitor | Monitor to mirror, as `rokast monitors` names it (X11 only; on Wayland the screen-sharing dialog asks) |
| `FIT` | `bars` | How the picture fits the TV (see [Picture size](#picture-size)): `bars` (scaled in, black bars when the shapes differ) or `match` (this computer's screen takes the TV's size while mirroring; X11 only) |
| `FPS` | `tv` | Frames a second (see [Smooth motion](#smooth-motion)): `tv` (what the TV asks for, 30 at HD on Roku TVs) or `60` |
| `SCREEN` | `mirror` | `mirror` (the TV shows this screen) or `extend` (the TV is a second screen; see [Second screen](#second-screen); X11 only) |
| `EXTEND_SIDE` | `right` | Where the second screen sits: `right`, `left`, `above` or `below` |
| `DISPLAY` | `:0` | X display to capture (X11 only) |
| `CHECK_UPDATES` | `1` | Once a day, ask GitHub for the latest release and show a notice if it is newer (`rokast version --check`); `0` turns it off. Only `rokast update` (or the app's **Update** button) downloads anything |
| `AUTO_UPDATE` | `0` | `1`: the desktop app installs a new release by itself when it starts (`rokast update --auto`), only for installs made by `scripts/install` and never while mirroring |

Every key can also be set as an environment variable, e.g. `BITRATE=12M scripts/rokast start`.

### Sound

By default the sound plays **only on the TV** while you mirror: once the TV starts playing, rokast adds a silent
output called *rokast (TV)*, makes it the default and moves the playing apps onto it, so the laptop speakers go
quiet and the TV gets no echo. When mirroring stops, everything goes back where it was. If rokast crashes, the next
`rokast start` puts it back, or run `rokast doctor --fix`.

`rokast audio` lists your outputs and the apps playing sound right now. With those names:

- `AUDIO=both`: the TV and this computer both play (how 0.5 and earlier worked: you hear it twice, slightly out of step)
- `AUDIO=app:Firefox`: only that app goes to the TV, the rest stays on the speakers (new tabs too)
- `AUDIO=device:<output>`: send whatever plays on that output, without moving anything
- `AUDIO=off`: video only

The app's **Sound** menu offers the same choices.

### Picture size

A laptop screen is often taller than a TV (16:10, e.g. 1920x1200, against the TV's 16:9 1920x1080). By default
(`FIT=bars`) rokast scales the picture in, so the TV shows thin black bars at the sides. With `FIT=match` (the app's
**Fill the TV** switch) the laptop screen switches to the TV's size once the TV starts playing, so the picture fills
the TV exactly, with nothing cut off or stretched; the laptop itself shows the TV's shape while you mirror. When
mirroring stops it switches back; after a crash the next `rokast start` or `rokast doctor --fix` does. X11 only (on
Wayland the bars stay), and only when the screen offers a mode of that size (`xrandr` lists them); otherwise rokast
says so and keeps the bars.

### Smooth motion

Roku TVs ask for HD at 30 frames a second at most, so fast movement (scrolling, dragging, games) looks a little
jerky. With `FPS=60` (the app's **Smooth motion** switch) rokast sends 60 frames a second anyway, labelled with the
H.264 level that needs (4.2 at 1080p); the TVs tested play it. It works when mirroring, with **Fill the TV** and with
**TV as a second screen**. On X11 the laptop screen also runs at 60 Hz while mirroring (switched back when you stop)
so frames come at even intervals; a fast panel's 165 Hz doesn't divide into 60. It uses a bit more Wi-Fi and
encoder time; if the picture stutters or freezes on your TV, set `FPS=tv`.

### Second screen

With `SCREEN=extend` (the app's **TV as a second screen** switch) the TV becomes an extra screen beside the laptop
instead of a copy of it: drag a window (or a video) to the right edge and it shows on the TV only, while the laptop
keeps its own desktop. `EXTEND_SIDE` puts it `left`, `above` or `below` instead. When mirroring stops the screen goes
away and windows on it come back to the laptop; after a crash the next `rokast start` or `rokast doctor --fix` removes it.

How: on X11 rokast makes a free video port of the laptop's Intel or AMD graphics look connected, with a screen
description of the TV's size (a kernel debug setting, written as root through the same `sudo` used for Wi-Fi), so
the desktop treats it like a real monitor. `rokast doctor` (with `SCREEN=extend`) says whether this works here:
**full**, or **basic** when there is no free port (NVIDIA-only laptops) or the kernel blocks debug settings
(lockdown under some Secure Boot setups). In basic mode the TV area exists and is shown, but windows don't open or
maximise there by themselves; drag them over. Wayland support is planned; there rokast mirrors for now.

### Updating

How you update depends on how you installed:

| Installed with | Update with |
|---|---|
| a release tarball or git clone + `scripts/install` | `rokast update` (or **Update** in the app). It downloads the latest release, refuses it unless its signature checks out against the key your install already has (`config/release-signers`), installs it next to the old version and switches over; `rokast update --rollback` goes back. It won't update while you are mirroring |
| the .deb or .rpm | `rokast update` downloads and checks the new package, then installs it through your package manager (it asks for your password) |
| the AUR | your AUR helper, e.g. `yay -Syu rokast` |
| running from a git clone without installing | `git pull` |

`rokast update --check` only says whether there is something newer. Updating from v0.3.0 or older: install the new
release by hand once (step 2 above); those versions don't have `rokast update` yet. Checking signatures needs
`ssh-keygen` (package `openssh`, or `openssh-client` on Debian/Ubuntu; the .deb and .rpm pull it in).

## Safety

- **Only your own TV can be selected.** A Wi-Fi Direct peer is only connectable if a Roku on your
  own LAN (answering Roku ECP on port 8060) reports the same name *and* a Wi-Fi MAC matching the
  peer's P2P address. Neighbours' TVs that are merely in radio range are never used.
- The RTSP listener is bound to the Wi-Fi Direct interface only (`SO_BINDTODEVICE`), never the LAN.
- DHCP on the TV link installs no default route and no DNS servers.
- Everything is removed on exit: the patched supplicant, the P2P interface and the DHCP client.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no p2p-dev-* interface` | NetworkManager's Wi-Fi P2P is unavailable; check `sudo ls /run/wpa_supplicant` and `iw list` for P2P support |
| `patched wpa_supplicant missing` | run `scripts/install` (or `scripts/build-supplicant`) |
| `selftest` FAIL with `h264_vaapi` / `VAAPI` errors | install your GPU's VA-API driver (see Limitations), or set `ENCODER=libx264` |
| `doctor`: supplicant built from an older patch | an upgrade changed the patch; run `scripts/build-supplicant` |
| `could not join the TV's Wi-Fi Direct group` | TV screen mirroring mode must be Prompt/Always allow; stand closer; retry |
| `the TV joined but never opened RTSP` | the TV's prompt was declined or timed out, or the device is in the TV's blocked list (Settings > System > Screen mirroring > Screen mirroring devices) |
| `no screen-sharing portal` | install `xdg-desktop-portal` and your desktop's backend; `rokast doctor` checks it |
| `screen sharing was cancelled` | the screen picker was closed; start again and pick a screen |
| Black picture on Wayland | force the portal path with `CAPTURE=wayland` (x11grab under XWayland sees only X11 windows) |
| Wrong screen on Wayland | delete `~/.config/rokast/portal-token` to get the picker again |
| Debug the link | `sudo cat /run/rokast/supplicant.log` while running |

## How it works (short)

1. NetworkManager's `wpa_supplicant` discovers the TV over Wi-Fi Direct and sends a push-button
   *provision discovery*, which opens a WPS window on the TV's always-on P2P group.
2. A second, patched `wpa_supplicant` on a dedicated P2P-client interface joins that group via
   WPS, sending the same P2P and Wi-Fi Display information elements an Android phone sends.
3. DHCP from the TV; the TV then opens RTSP to us on port 7236 (the Miracast M1–M7 handshake).
4. `ffmpeg` captures the screen (X11: `x11grab`; Wayland: portal + PipeWire via GStreamer) and
   audio, encodes H.264/AAC and sends RTP/MPEG-TS to the TV.

The full write-up, including what did *not* work and why, is in
[docs/how-it-works.md](docs/how-it-works.md).

## Development

```bash
bash tests/check.sh                         # before pushing: fmt, clippy, tests, parity, app tests, shellcheck (as CI)
cargo test --workspace                     # RTSP handshake vs a fake Roku sink, IE bytes, command builders, allowlist, paths, selftest, update
tests/parity/run.sh; tests/parity/cli.sh  # same answers and output as before the Rust port (frozen in tests/parity)
(cd ui && cargo test)                      # the desktop app
bash tests/test_install.sh                 # installer: upgrade, rollback, uninstall in a throwaway HOME
bash tests/test_monitor_capture.sh         # MONITOR= captures the right monitor (Xvfb + ffmpeg + ImageMagick; not in CI)
bash tests/test_fit_live.sh                # FIT=match on the real screen: switch, restore on stop and after a crash (not in CI)
bash tests/test_extend_live.sh             # SCREEN=extend on the real screen: second screen added, GDK + grab, removed on stop and after a crash (sudo; not in CI)
bash tests/test_pkg_deb.sh <deb>; bash tests/test_pkg_rpm.sh <rpm>   # packages in clean containers (docker; CI runs both)
bash tests/live_join_soak.sh 10 15         # 10 cold-start joins with the real TV idle 15 min between (not in CI)
```

Releases: add a `## X.Y.Z` section to `CHANGELOG.md`, then `scripts/release X.Y.Z`; GitHub Actions builds and publishes
the release (see `playbooks/release.md`).

Layout: `src/rokast-core/` (the library: p2p, link, wfd, rtsp, capture, wayland, audio, fit, extend, edid, paths, selftest, update),
`src/rokast/` (the `rokast` command), `ui/` (Rust GTK 4 desktop app),
`patches/` (wpa_supplicant patch + build config), `scripts/` (entry points), `playbooks/` (operational runbook).

`rokast start` prints machine-readable progress lines, `status: scanning|sharing|joining|waiting|streaming|stopped|error [detail]`,
which the desktop app follows; other front ends can use them too.

## License

MIT for this project's code (see [LICENSE](LICENSE)). `patches/hostap-2.11-roku.patch` modifies
[hostap](https://w1.fi/) and is distributed under hostap's BSD license; `scripts/build-supplicant`
downloads hostap from its upstream repository at build time.

Not affiliated with Roku, Inc. "Roku" is a trademark of Roku, Inc.; "Miracast" and "Wi-Fi Direct"
are trademarks of the Wi-Fi Alliance.
