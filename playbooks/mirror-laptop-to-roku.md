# mirror-laptop-to-roku

when: user wants the laptop screen (and audio) on a Roku TV with nothing installed on the TV, or the mirror is broken
tools: scripts/rokast (src/miracast/{p2p,wfd,capture,cli}.py), scripts/build-supplicant (patches/), scripts/install, ui/ (GTK4 Rust app), paths in src/miracast/paths.py
cost: free

## How it works

Native Miracast (Wi-Fi Display), same as Windows/Android "Cast":
1. NetworkManager's wpa_supplicant (`p2p-dev-wlo1`) finds the TV and sends a PBC provision discovery -> the TV opens a WPS push-button window on its always-on P2P group.
2. A **patched** wpa_supplicant (ours, `/run/rokast`) on our own P2P-client iface `p2p-wlo1-mir` joins that group by WPS, with a phone-style P2P IE + WFD IE in the assoc request (p2p.py).
3. dhcpcd on the group iface (no gateway, no DNS). The TV connects to OUR :7236 ~0.5 s after its lease; RTSP M1..M7 (wfd.py).
4. x11grab (X11) or portal ScreenCast + PipeWire via gst-launch pipewiresrc -> raw I420 on ffmpeg stdin (Wayland, wayland.py) + NVENC -> RTP/MPEG-TS to the TV (capture.py).

## Preconditions

- X11 session (XFCE here), or Wayland with xdg-desktop-portal + backend, gst-plugin-pipewire, python-gobject. Wi-Fi chip with P2P (Intel AX211: yes; `iw list | rg P2P`).
- NetworkManager's wpa_supplicant exposes `p2p-dev-<wifi>` (check: `sudo ls /run/wpa_supplicant`).
- Patched supplicant built: `scripts/install` or `scripts/build-supplicant` (clones hostap_2_11 into `~/.cache/rokast/hostap-src`, applies `patches/hostap-2.11-roku.patch`, installs to `~/.local/share/rokast/supplicant/` with a `patch.sha256` stamp; `doctor` FAILs when the stamp no longer matches the patch).
- Roku: Settings > System > Screen mirroring = "Prompt" or "Always allow".
- `scripts/rokast doctor` prints only `ok`.

## Steps

1. `scripts/rokast list` -> the TV name (only Rokus answering on OUR LAN are listed; neighbours' TVs are unselectable by design).
2. `scripts/rokast start` (TARGET from `~/.config/rokast/rokast.conf` (`rokast config` prints it), or `start "<name>"`). Accept the prompt on the TV if shown. ~20 s to picture.
3. Ctrl+C (or SIGTERM) stops mirroring, stops our supplicant, removes the group iface.

Desktop app: `scripts/install` once (cargo build or prebuilt + menu entry), then `rokast ui`. It runs `rokast list --json` and
`rokast start <tv>` (settings passed as RES/BITRATE/AUDIO env), follows the `status: <state>` lines, and stops with SIGTERM
(also on Quit, waiting for teardown). Keep the status line names stable: the UI keys off them.
Tray icon (ui/src/tray.rs, ksni StatusNotifierItem, icon drawn in code: grey idle, amber connecting, green mirroring, red error):
left click toggles the window; menu = status, Start/Stop, Show/Hide, Quit. With a tray, closing the window only hides it
(mirroring continues); without a StatusNotifier host the tray is skipped and close stops+quits as before. `rokast ui --tray` starts hidden.

## Verify

- Install/upgrade: `bash tests/test_install.sh` (scratch HOME: v0.1.0 layout -> upgrade, prune, rollback, uninstall).
  Real layout: `~/.local/share/rokast/{versions/<v>,current,previous,supplicant}`, `~/.local/bin/rokast -> current/scripts/rokast`.

- `python3 -I -m unittest discover -s tests` (handshake vs fake Roku sink, IE bytes vs the working phone, command builders, allowlist).
- Live: `link up: p2p-wlo1-mir ...`, `wfd: sink ... connected`, `wfd: negotiated 1920x1080@30 ...`, ffmpeg line; TV shows the screen;
  `curl -s <tv-ip>:8060/query/active-app` -> RokuCast.
- After stop: `iw dev` shows only wlo1; no `~/.local/share/rokast/supplicant` process; wlo1 keeps its LAN IP.

## Why each piece exists (do not "simplify" these away)

- NM's own `p2p_connect <mac> pbc join` joins and gets DHCP but the Roku NEVER opens RTSP (days of IE/name/type variants: no effect).
  Joining from our own supplicant with the P2P IE in the assoc request works.
- Without a P2P IE in the assoc request the TV deauths (reason 1) right after association.
- The group PSK rotates per group instance -> stored credentials don't survive; WPS every time. The PD from p2p-dev is what opens the TV's PBC window (single enrollee, short).
- Patches (patches/hostap-2.11-roku.patch): keep a pre-created `__p2pcl` iface in P2P-client mode (driver init + `WPA_FORCE_P2P_GROUP` at associate);
  null-check `global->p2p` in the WPS credential path (segfault right after WPS-CRED-RECEIVED when `p2p_disabled=1`).
- `iw ... interface add ... type __p2pcl` alone yields managed mode; `iw dev X set type __p2pcl` is required.
- Chip allows ONE P2P-client/GO iface: NM's group iface and ours cannot coexist; disconnect() removes all `p2p-<wifi>-*`.
- Listener binds to the group iface (SO_BINDTODEVICE) BEFORE dhcp: the TV retries only ~4 s after its lease; not reachable from the LAN.
- dhcpcd `--nogateway --nohook resolv.conf`: LAN default route and DNS stay on wlo1.

## Notes

- NAME default = pretty hostname/hostname (`rokast name`); the TV accepted a renamed source without blocking (tested "BlackBox").
- Wayland test without a Wayland desktop: headless sway (WLR_BACKENDS=headless WLR_RENDERER=pixman) in `dbus-run-session`, own XDG_CONFIG_HOME with
  xdg-desktop-portal-wlr `chooser_type=none output_name=HEADLESS-1`, start xdg-desktop-portal-wlr + xdg-desktop-portal, put a Wayland window up
  (swaybg is not a dependency: an empty output captures black), then `rokast start`. Portal open happens BEFORE the join (status `sharing`).
- UI test without clicking by hand: launch with DISPLAY=:0, `xdotool search --name '^rokast$'`, `import -window` for screenshots, `xdotool mousemove --window W x y click 1`.
- Tray test without clicking: bus name `org.kde.StatusNotifierItem-<pid>-1`; `gdbus call --session -d $N -o /StatusNotifierItem -m org.kde.StatusNotifierItem.Activate 0 0`
  (left click); menu: `... -o /MenuBar -m com.canonical.dbusmenu.GetLayout -- 0 -1 '["label"]'` and `... Event -- <id> clicked '<0>' 0`.
  Close the window with `wmctrl -i -c <id>` (a real close request); `xdotool windowclose` destroys the X window and breaks GTK.

- Do not derive the LAN from the default route (a VPN may own it). Use the Wi-Fi interface address.
- P2P address of a Roku = its wifi MAC with first octet |= 0x02 (4c: -> 4e:). The allowlist relies on this. Group BSSID comes from `p2p_peer` `interface_addr`.
- `$DISPLAY` is often `:0.0`; capture strips the screen suffix. The agent shell has no X auth env; cli sets DISPLAY/XAUTHORITY for ffmpeg.
- Audio is only sent when the TV offers AAC (WFD LPCM framing is not what ffmpeg's pcm muxing produces).
- Debug: `sudo cat /run/rokast/supplicant.log` (WPS-CRED-RECEIVED, COMPLETED); kernel `segfault` lines in `journalctl -k` mean the patch is missing.
- Backgrounded from a non-interactive shell, SIGINT is ignored; use SIGTERM to stop it in tests.
- Fallbacks if a TV firmware update breaks this: HDMI cable, or a ~$25 Miracast dongle.
