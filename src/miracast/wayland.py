"""Wayland screen capture through the xdg-desktop-portal ScreenCast API + PipeWire.

ffmpeg has no PipeWire input, so GStreamer's pipewiresrc delivers raw frames (already scaled to the
negotiated mode, letterboxed, constant frame rate) to ffmpeg's stdin; the encode/RTP side is the same
as on X11. Works on any desktop with a ScreenCast portal backend (GNOME, KDE, wlroots: sway, Hyprland...).

The portal asks the user to pick a screen, so open() runs BEFORE joining the TV (its RTSP window is
short). With portal v4+ the choice is remembered (restore token) and later runs don't ask again.
"""
import os
import secrets

BUS = "org.freedesktop.portal.Desktop"
OBJ = "/org/freedesktop/portal/desktop"
IFACE = "org.freedesktop.portal.ScreenCast"
TOKEN_FILE = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"), "rokast", "portal-token")
MONITOR = 1                  # source type
CURSOR_EMBEDDED, CURSOR_HIDDEN = 2, 1
PERSIST_UNTIL_REVOKED = 2


def is_wayland(env=os.environ):
    """A Wayland session (XWayland's $DISPLAY only sees X11 windows, so x11grab would capture black)."""
    return bool(env.get("WAYLAND_DISPLAY")) or env.get("XDG_SESSION_TYPE") == "wayland"


def gst_cmd(fd, node, width, height, fps):
    """Pure function: PipeWire stream -> raw I420 frames of exactly width x height @ fps on stdout. Unit-tested."""
    return ["gst-launch-1.0", "-q",
            # keepalive: PipeWire only delivers frames on damage; resend the last one so a static screen keeps flowing
            "pipewiresrc", f"fd={fd}", f"path={node}", "do-timestamp=true", "keepalive-time=100", "always-copy=true",
            "!", "videoconvert", "!", "videoscale", "add-borders=true", "!", "videorate",
            "!", f"video/x-raw,format=I420,width={width},height={height},framerate={fps}/1,pixel-aspect-ratio=1/1",
            "!", "fdsink", "fd=1", "sync=false"]


class ScreenCast:
    """One portal session. open() -> (pipewire fd, node id); keep the object alive while streaming."""

    def __init__(self):
        import gi
        gi.require_version("Gio", "2.0")
        from gi.repository import Gio, GLib
        self.Gio, self.GLib = Gio, GLib
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.sender = self.bus.get_unique_name()[1:].replace(".", "_")
        self.session = None
        self.fd = None
        self.node = None

    def _prop(self, name):
        GLib = self.GLib
        r = self.bus.call_sync(BUS, OBJ, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", (IFACE, name)),
                               GLib.VariantType("(v)"), self.Gio.DBusCallFlags.NONE, 5000, None)
        return r.unpack()[0]

    def _request(self, method, args, options, timeout=120):
        """Call a portal method that answers through a Request object's Response signal."""
        GLib, Gio = self.GLib, self.Gio
        token = "rokast" + secrets.token_hex(4)
        path = f"{OBJ}/request/{self.sender}/{token}"
        options = dict(options, handle_token=GLib.Variant("s", token))
        loop, out = GLib.MainLoop(), {}

        def on_response(_c, _s, _p, _i, _sig, params):
            out["code"], out["results"] = params.unpack()
            loop.quit()

        sub = self.bus.signal_subscribe(BUS, "org.freedesktop.portal.Request", "Response", path, None,
                                        Gio.DBusSignalFlags.NONE, on_response)
        try:
            sig = "(" + "".join(t for t, _ in args) + "a{sv})"
            self.bus.call_sync(BUS, OBJ, IFACE, method, GLib.Variant(sig, tuple(v for _, v in args) + (options,)),
                               GLib.VariantType("(o)"), Gio.DBusCallFlags.NONE, 10000, None)
            GLib.timeout_add_seconds(timeout, loop.quit)
            loop.run()
        finally:
            self.bus.signal_unsubscribe(sub)
        if "code" not in out:
            raise RuntimeError(f"screen-sharing portal: no answer to {method} in {timeout}s")
        if out["code"] == 1:
            raise RuntimeError("screen sharing was cancelled")
        if out["code"] != 0:
            raise RuntimeError(f"screen-sharing portal refused {method} (code {out['code']})")
        return out["results"]

    def open(self):
        GLib = self.GLib
        try:
            version = self._prop("version")
            cursors = self._prop("AvailableCursorModes")
        except GLib.Error as e:
            raise RuntimeError(f"no screen-sharing portal (install xdg-desktop-portal + a backend for your desktop): {e.message}")
        res = self._request("CreateSession", [], {"session_handle_token": GLib.Variant("s", "rokast" + secrets.token_hex(4))})
        self.session = res["session_handle"]
        opts = {"types": GLib.Variant("u", MONITOR), "multiple": GLib.Variant("b", False),
                "cursor_mode": GLib.Variant("u", CURSOR_EMBEDDED if cursors & CURSOR_EMBEDDED else CURSOR_HIDDEN)}
        if version >= 4:
            opts["persist_mode"] = GLib.Variant("u", PERSIST_UNTIL_REVOKED)
            try:
                opts["restore_token"] = GLib.Variant("s", open(TOKEN_FILE).read().strip())
            except OSError:
                pass
        self._request("SelectSources", [("o", self.session)], opts)
        res = self._request("Start", [("o", self.session), ("s", "")], {})
        streams = res.get("streams") or []
        if not streams:
            raise RuntimeError("screen-sharing portal returned no stream")
        self.node = streams[0][0]
        if res.get("restore_token"):
            os.makedirs(os.path.dirname(TOKEN_FILE), exist_ok=True)
            with open(TOKEN_FILE, "w") as f:
                f.write(res["restore_token"])
        r, fds = self.bus.call_with_unix_fd_list_sync(
            BUS, OBJ, IFACE, "OpenPipeWireRemote", GLib.Variant("(oa{sv})", (self.session, {})),
            GLib.VariantType("(h)"), self.Gio.DBusCallFlags.NONE, 10000, None, None)
        self.fd = fds.get(r.unpack()[0])
        os.set_inheritable(self.fd, True)
        return self.fd, self.node

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
        if self.session:
            try:
                self.bus.call_sync(BUS, self.session, "org.freedesktop.portal.Session", "Close", None, None,
                                   self.Gio.DBusCallFlags.NONE, 5000, None)
            except self.GLib.Error:
                pass
            self.session = None
