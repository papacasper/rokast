# How rokast works (and why it took a patched wpa_supplicant)

This explains how a Linux machine becomes a Miracast *source* that a Roku TV accepts, and the
dead ends along the way. The short version: a Roku only starts a Miracast session for a source
that joins its Wi-Fi Direct group the way an Android phone does. The standard Linux join path
gets a working link, but the TV never opens the stream. A standalone, lightly patched
`wpa_supplicant` running on a dedicated P2P-client interface does what the phone does.

## 1. Miracast in one page

Miracast (Wi-Fi Display, WFD) has three layers:

1. **Wi-Fi Direct (P2P) link.** Source and sink form a P2P group. One side is the group owner
   (GO, an access point), the other a client. Devices advertise WFD capabilities in a *WFD
   information element* (IE) in their probe, provision-discovery and association frames.
2. **RTSP control session (TCP 7236).** The *sink connects to the source*. The source sends
   M1 (OPTIONS), the sink answers and sends M2. The source then reads the sink's capabilities
   (M3 GET_PARAMETER), picks a format (M4 SET_PARAMETER) and triggers setup (M5). The sink sends
   SETUP (M6) and PLAY (M7). GET_PARAMETER is used as a keep-alive.
3. **Media.** H.264 video, plus optional AAC/LPCM audio, in MPEG-TS over RTP/UDP, sent to the
   sink's RTP port.

Layers 2 and 3 are plain networking and straightforward. In this project they live in
`src/rokast-core/src/{wfd,rtsp,capture}.rs` and are unit-tested against a fake sink. Layer 1 is where
Roku compatibility is decided.

## 2. How a Roku behaves

- A Roku runs an **always-on, autonomous P2P group**: it is GO all the time, with an SSID like
  `DIRECT-roku-XXX-YYYYYY`. It does not answer GO negotiation, so sources must *join* its group.
- Its P2P device address is its Wi-Fi MAC with the locally-administered bit set
  (`4c:..` -> `4e:..`). The group BSSID (`interface_addr` in `wpa_cli p2p_peer`) is different again.
- Joining uses WPS push-button (PBC). The TV opens a short PBC window, for a single enrollee,
  when it receives a **provision discovery request (PBC, join)** from the source.
- **The group passphrase changes every time the group is re-created.** Credentials from a
  previous WPS run don't work after the TV restarts its group, so every session has to run WPS.
- After a client gets its DHCP lease, the TV opens TCP to the client's port 7236 about 0.5 s
  later. It retries for only about 4 s (3 SYNs). If nothing is listening by then, it gives up for
  that association.
- The TV checks the client's **association request**. A client with no P2P IE is
  de-authenticated (reason 1) right after associating.

## 3. What a working phone does

A Samsung phone mirroring to the same TV was captured over the air, with the laptop's own Wi-Fi
card in monitor mode pinned to the group's channel. Its association request contains:

- a **P2P IE**: P2P capability, then device info (a random P2P device address, config methods
  `0x0188`, primary device type `10-0050F204-5` = telephone/smartphone, and the device name)
- a **WFD IE**: device information subelement with type *source*, session available, RTSP port
  7236, max throughput 50 Mbps (`00 0006 0110 1c44 0032`)
- two small vendor IEs (`00:00:f0` and `50:6f:9a` type `0x23`)
- no WPS IE on a repeat join, because it has credentials stored from earlier in the same group's life

The phone's Wi-Fi Direct code is stock Android (AOSP `wpa_supplicant`). Nothing proprietary
happens before RTSP.

## 4. What did not work

The obvious approach uses the `wpa_supplicant` that NetworkManager already runs (its
`p2p-dev-wlan0` interface):

```
wpa_cli -i p2p-dev-wlan0 p2p_connect <tv-p2p-addr> pbc join
```

NetworkManager must also own a `wifi-p2p` connection, or it deletes the group interface. This
joins the group: WPS succeeds, DHCP succeeds, and there is a working IP link to the TV. **But the
TV never opens TCP to port 7236.** None of these changed that:

- device name, primary device type, config methods
- WFD IE variants (`0x0090`, `0x0110`, different throughput values), verified on air
- the phone's extra vendor IEs, injected with `vendor_elem_add 13` on the group interface plus a
  reassociate, and verified present in the association request on air
- the persistent-group flag, `p2p_invite`, persistent P2P device addresses

Why it fails is not fully pinned down. The difference that turned out to matter is *who* builds
the association request, and in which interface mode. In NM's instance the group interface is
created and driven by the P2P state machine. A dedicated client instance (below) sends exactly
the phone-style IEs in the association request on a P2P-client interface, and the TV accepts that.

## 5. What works

`src/rokast-core/src/link.rs`, `connect()` (with `p2p.rs` and `lan.rs`):

1. **Allowlist** (`lan_rokus()`, `pick()`). Find Rokus on the LAN through Roku ECP
   (`http://<ip>:8060/query/device-info`). Only a TV whose name *and* derived P2P address match a
   Wi-Fi Direct peer can be selected, so a neighbour's TV in radio range is never chosen.
2. **Discovery + PBC trigger.** Using NetworkManager's existing `p2p-dev-*` interface: run
   `p2p_find` until the TV shows up (`p2p_peer` gives `interface_addr`, the group BSSID), then
   `p2p_prov_disc <tv> pbc join`. The TV replies with a PBC response and its group starts
   advertising `[WPS-PBC]`.
3. **Own P2P-client interface.**
   `iw dev wlan0 interface add p2p-wlan0-mir type __p2pcl`, followed by
   `iw dev p2p-wlan0-mir set type __p2pcl` (the add alone leaves it in managed mode). It gets a
   stable locally-administered MAC and is marked unmanaged in NetworkManager. The chip allows only
   one P2P-client/GO interface, so any leftover `p2p-wlan0-*` interfaces are removed first.
4. **Patched standalone `wpa_supplicant`** on that interface (`p2p_disabled=1`, control socket in
   `/run/rokast`). Before connecting, the P2P IE, WFD IE and extra vendor IEs are installed
   with `vendor_elem_add 13` (frame type 13 = P2P association request), so they are in the very
   first association. Then `wps_pbc <group-bssid>`. WPS hands over the current passphrase, the
   supplicant reassociates with it, and the state reaches `COMPLETED`.
5. **Listen, then DHCP.** The RTSP listener is opened *before* DHCP, bound to the P2P interface
   with `SO_BINDTODEVICE`, so it is already listening when the lease lands and is never reachable
   from the LAN. Then `dhcpcd -4 -1 --nogateway --nohook resolv.conf` gets a lease from the TV
   without touching the default route or DNS.
6. **RTSP + media.** The TV connects within about 0.5 s. The M1–M7 exchange negotiates (for a
   G512X) 1920x1080p30 H.264 High@4.0 with AAC, and `ffmpeg` streams `x11grab` + PulseAudio
   monitor → H.264 (NVENC/VAAPI/x264) + AAC → RTP/MPEG-TS to the TV's RTP port (1028 on Roku).
   Sound (`src/rokast-core/src/audio.rs`): only when the TV starts playing, `pactl load-module module-null-sink
   sink_name=rokast` adds a silent output; `AUDIO=tv` makes it the default and moves every stream onto it,
   `app:<name>` moves only that app's streams (and, through `pactl subscribe`, each new one), and ffmpeg records
   `rokast.monitor`. What was changed is saved in `$XDG_RUNTIME_DIR/rokast/audio.json` before changing it, so stop,
   SIGTERM, and the next start after a crash can undo it. PipeWire accepts a move to a sink it has only just
   created and then drops it, so moves are checked and retried for a second.
   On Wayland, `src/rokast-core/src/wayland.rs` opens an xdg-desktop-portal ScreenCast session before
   joining (the user may have to pick a screen, and the TV's RTSP window is too short to wait for
   that). It passes the PipeWire fd to GStreamer (`pipewiresrc ! videoconvert ! videoscale ! videorate`),
   which writes raw I420 frames at exactly the negotiated size and rate into ffmpeg's stdin. The
   encode and RTP side is unchanged. `keepalive-time` makes PipeWire repeat the last frame on a static
   screen, so the TV keeps getting a steady stream.
7. **Picture options (X11).** All change the screen only once the TV plays, save what they changed in
   `$XDG_RUNTIME_DIR/rokast/` first (`fit.json`, `extend.json`) and undo it on stop, SIGTERM, the next start after a
   crash (before the screen is measured, or the capture area would be wrong) or `rokast doctor --fix`.
   - `FIT=match` (`fit.rs`): `xrandr --output <laptop> --mode <TV size>`, so a 16:10 laptop fills a 16:9 TV.
   - `SCREEN=extend` (`extend.rs`, `edid.rs`): a free Intel/AMD video port (HDMI first) is made to look connected by
     writing a generated EDID of the TV's size to its debugfs `edid_override` and `on` to `force`
     (`/sys/kernel/debug/dri/<n>/<port>/`, as root), then a hotplug; `xrandr` puts the new output beside the laptop
     and ffmpeg captures its area. No free port, or debugfs blocked by lockdown: `--fb` plus `--setmonitor` makes a
     "basic" area of the framebuffer instead (shown, but windows don't open or maximise there by themselves).
   - `FPS=60` (`capture::smooth`): Roku TVs offer HD only up to 30 fps (CEA bitmap `0x0001d4bf`) but play 60. The
     negotiated mode is kept, ffmpeg captures and encodes at 60 and labels the stream with the H.264 level that rate
     needs (1080p60: 4.2; nvenc refuses 60 at 4.0). When mirroring, the laptop goes to its ~60 Hz rate so frames are
     evenly spaced (165 Hz does not divide by 60).

Stopping removes everything: the supplicant gets `terminate`, the DHCP client is released and the
interface is deleted.

## 6. The wpa_supplicant patch

`patches/hostap-2.11-roku.patch` (applied by `scripts/build-supplicant` on top of `hostap_2_11`):

1. **`driver_nl80211.c`, driver init.** Stock `wpa_supplicant` switches any interface it is
   started on to STATION mode. The patch keeps an interface that is already `P2P_CLIENT` in that
   mode, the same way mesh interfaces are kept.
2. **`wpa_supplicant.c`, `wpa_supplicant_associate()`.** When `WPA_FORCE_P2P_GROUP` is set in the
   environment, every network is treated as a P2P group (`ssid->p2p_group = 1`). The association
   parameters then carry `p2p=1` and the nl80211 driver keeps the interface in P2P-client mode
   instead of reverting to station mode. Only rokast's instance sets this variable.
3. **`wps_supplicant.c`, credential handling.** Fixes a crash: with a P2P group network and P2P
   disabled, `is_p2p_6ghz_capable(wpa_s->global->p2p)` dereferences NULL right after
   `WPS-CRED-RECEIVED`. A NULL check is added. This is arguably an upstream bug.

The patched binary is only ever run by rokast, with its own config and control directory.
It doesn't replace or touch the system `wpa_supplicant`.

## 7. Debugging techniques that paid off

- **Monitor capture on the group's channel with the same card.** Hopping monitors miss the 0.2 s
  join. Switching the Wi-Fi interface itself to monitor mode, pinned to the TV group's frequency,
  captured the phone's full probe/auth/assoc/EAPOL sequence for byte-level comparison.
- **IE byte-equality test.** a unit test in `src/rokast-core/src/p2p.rs` checks that `assoc_ies()` produces exactly the
  phone's IE layout. It caught a wrong device-category encoding and a WFD subelement framing bug
  that the TV happened to tolerate.
- **`addr2line` on the kernel segfault line** (`journalctl -k`) located the hostap NULL
  dereference in seconds, without a core dump.
- **tcpdump on the P2P interface right after DHCP** showed the TV's SYNs to 7236 being reset.
  That is what revealed the ~4 s retry window and why the listener must start first.

## 8. Known gaps / ideas

- Wayland: tested on sway; GNOME/KDE portals untested. `FIT`, `SCREEN=extend` and the 60 Hz switch are X11 only
  (planned: the portal's `VIRTUAL` source for a second screen).
- Test VAAPI on real Intel/AMD hardware; test non-Roku sinks.
- UIBC (remote input from the TV) is not implemented.
- If a future Roku OS rejects these IEs, re-capture a working phone and update `assoc_ies()`.
  The test pins the layout.
