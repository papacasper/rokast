//! Wayland screen capture through the xdg-desktop-portal ScreenCast API + PipeWire.
//!
//! ffmpeg has no PipeWire input, so GStreamer's pipewiresrc delivers raw frames (already scaled to the negotiated
//! mode, letterboxed, constant frame rate) to ffmpeg's stdin; the encode/RTP side is the same as on X11. Works on any
//! desktop with a ScreenCast portal backend (GNOME, KDE, wlroots: sway, Hyprland...).
//!
//! The portal asks the user to pick a screen, so `open` runs BEFORE joining the TV (its RTSP window is short). With
//! portal v4+ the choice is remembered (restore token) and later runs don't ask again.
//!
//! SCREEN=extend: asked for source type VIRTUAL, the portal has the compositor add a new screen and streams that
//! (GNOME's and KDE's portals; others list only MONITOR/WINDOW in AvailableSourceTypes and get a copy instead).
use crate::stop::{self, Fail};
use crate::Env;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

const BUS: &str = "org.freedesktop.portal.Desktop";
const OBJ: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.ScreenCast";
const MONITOR: u32 = 1; // source types
const VIRTUAL: u32 = 4;
const CURSOR_EMBEDDED: u32 = 2;
const CURSOR_HIDDEN: u32 = 1;
const PERSIST_UNTIL_REVOKED: u32 = 2;

/// Where the portal's restore token is kept: $XDG_CONFIG_HOME/rokast/portal-token, or portal-token-virtual for a
/// virtual screen (a monitor's token must not restore a virtual session, nor the reverse).
pub fn token_file(env: &Env, virt: bool) -> PathBuf {
    let cfg = env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || PathBuf::from(env.get("HOME").cloned().unwrap_or_default()).join(".config");
    cfg.unwrap_or_else(home).join("rokast").join(if virt { "portal-token-virtual" } else { "portal-token" })
}

/// The size to ask for a virtual screen, before the TV has said what it plays: the first RES preference's
/// (720p.. -> 1280x720, else 1920x1080; every Roku tested plays 1080p). ffmpeg scales if the TV picks another.
pub fn virtual_size(res: &str) -> (u32, u32) {
    match res.split(',').next().map(str::trim) {
        Some(r) if r.starts_with("720") => (1280, 720),
        _ => (1920, 1080),
    }
}

/// What SCREEN=extend got on Wayland, for the log.
pub fn extend_note(virt: Option<(u32, u32)>) -> String {
    match virt {
        Some((w, h)) => format!("extend: the TV is a new {w}x{h} screen from the desktop (virtual monitor)"),
        None => {
            "extend: this desktop's screen-sharing portal can't add a screen (no virtual monitors); mirroring".into()
        }
    }
}

/// The source type to ask for: a virtual screen when wanted and the portal offers one (AvailableSourceTypes bitmask).
pub fn source_type(want_virtual: bool, available: u32) -> u32 {
    if want_virtual && available & VIRTUAL != 0 {
        VIRTUAL
    } else {
        MONITOR
    }
}

fn token() -> String {
    format!("rokast{:08x}", RandomState::new().build_hasher().finish() as u32)
}

type Results = HashMap<String, OwnedValue>;

/// One portal session; keep it alive while streaming (dropping it closes the session and the PipeWire fd).
pub struct ScreenCast {
    bus: Connection,
    sender: String,
    session: Option<OwnedObjectPath>,
    pub fd: Option<OwnedFd>,
    pub node: u32,
    /// Some(size) when the stream is a virtual screen the compositor added (SCREEN=extend).
    pub virt: Option<(u32, u32)>,
}

impl ScreenCast {
    pub fn new() -> Result<Self, String> {
        let bus = Connection::session().map_err(|e| format!("no D-Bus session bus: {e}"))?;
        let sender = bus.unique_name().map(|n| n.as_str()[1..].replace('.', "_")).unwrap_or_default();
        Ok(ScreenCast { bus, sender, session: None, fd: None, node: 0, virt: None })
    }

    /// A ScreenCast property (version, AvailableCursorModes).
    pub fn prop(&self, name: &str) -> Result<u32, String> {
        let r = self
            .bus
            .call_method(Some(BUS), OBJ, Some("org.freedesktop.DBus.Properties"), "Get", &(IFACE, name))
            .map_err(|e| e.to_string())?;
        let v: OwnedValue = r.body().deserialize().map_err(|e| e.to_string())?;
        let v = match &*v {
            Value::Value(inner) => inner.try_clone().map_err(|e| e.to_string())?.try_into(),
            _ => v.try_clone().map_err(|e| e.to_string())?.try_into(),
        };
        v.map_err(|e: zbus::zvariant::Error| format!("{name}: {e}"))
    }

    /// Call a portal method that answers through a Request object's Response signal.
    fn request<B>(&self, method: &str, body: impl FnOnce(String) -> B, timeout: u64) -> Result<Results, Fail>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let tok = token();
        let path = format!("{OBJ}/request/{}/{tok}", self.sender);
        let err = |e: zbus::Error| Fail::Err(format!("screen-sharing portal: {method}: {e}"));
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BUS)
            .and_then(|b| b.interface("org.freedesktop.portal.Request"))
            .and_then(|b| b.member("Response"))
            .and_then(|b| b.path(path.as_str()))
            .map_err(err)?
            .build();
        let signals = MessageIterator::for_match_rule(rule, &self.bus, Some(4)).map_err(err)?; // before the call
        self.bus.call_method(Some(BUS), OBJ, Some(IFACE), method, &body(tok)).map_err(err)?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Some(m) = signals.into_iter().next() {
                let _ = tx.send(m.and_then(|m| m.body().deserialize::<(u32, Results)>()));
            }
        });
        let end = Instant::now() + Duration::from_secs(timeout);
        let (code, results) = loop {
            stop::check()?;
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(r) => break r.map_err(err)?,
                Err(RecvTimeoutError::Timeout) if Instant::now() < end => {}
                Err(_) => return Err(format!("screen-sharing portal: no answer to {method} in {timeout}s").into()),
            }
        };
        match code {
            0 => Ok(results),
            1 => Err("screen sharing was cancelled".into()),
            c => Err(format!("screen-sharing portal refused {method} (code {c})").into()),
        }
    }

    /// Ask for a screen (the portal may show its picker) and open the PipeWire stream: `fd` and `node` are set.
    /// `virt`: a new screen of that size instead, if the portal can (then `self.virt` is set; else a picked monitor).
    pub fn open(&mut self, env: &Env, virt: Option<(u32, u32)>) -> Result<(), Fail> {
        let props = self.prop("version").and_then(|v| Ok((v, self.prop("AvailableCursorModes")?)));
        let (version, cursors) = props.map_err(|e| {
            format!("no screen-sharing portal (install xdg-desktop-portal + a backend for your desktop): {e}")
        })?;
        let session_tok = token();
        let res =
            self.request(
                "CreateSession",
                |t| {
                    (HashMap::from([
                        ("handle_token", Value::from(t)),
                        ("session_handle_token", Value::from(session_tok)),
                    ]),)
                },
                120,
            )?;
        let handle = res.get("session_handle").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        let handle = handle.ok_or("screen-sharing portal: CreateSession gave no session_handle")?;
        let session = OwnedObjectPath::try_from(handle).map_err(|e| e.to_string())?;
        self.session = Some(session.clone());
        let cursor = if cursors & CURSOR_EMBEDDED != 0 { CURSOR_EMBEDDED } else { CURSOR_HIDDEN };
        let kind = source_type(virt.is_some(), self.prop("AvailableSourceTypes").unwrap_or(MONITOR));
        self.virt = virt.filter(|_| kind == VIRTUAL);
        let file = token_file(env, self.virt.is_some());
        let restore = std::fs::read_to_string(&file).ok().map(|t| t.trim().to_string());
        let s = session.clone();
        self.request(
            "SelectSources",
            move |t| {
                let mut o = HashMap::from([
                    ("handle_token", Value::from(t)),
                    ("types", Value::from(kind)),
                    ("multiple", Value::from(false)),
                    ("cursor_mode", Value::from(cursor)),
                ]);
                if version >= 4 {
                    o.insert("persist_mode", Value::from(PERSIST_UNTIL_REVOKED));
                    if let Some(r) = restore {
                        o.insert("restore_token", Value::from(r));
                    }
                }
                (s, o)
            },
            120,
        )?;
        let s = session.clone();
        let res = self.request("Start", move |t| (s, "", HashMap::from([("handle_token", Value::from(t))])), 120)?;
        let streams = res.get("streams").and_then(|v| Vec::<(u32, Results)>::try_from(v.try_clone().ok()?).ok());
        self.node =
            streams.and_then(|s| s.first().map(|(n, _)| *n)).ok_or("screen-sharing portal returned no stream")?;
        if let Some(t) = res.get("restore_token").and_then(|v| String::try_from(v.try_clone().ok()?).ok()) {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&file, t);
        }
        let opts: HashMap<&str, Value> = HashMap::new();
        let r = self
            .bus
            .call_method(Some(BUS), OBJ, Some(IFACE), "OpenPipeWireRemote", &(&session, opts))
            .map_err(|e| format!("screen-sharing portal: OpenPipeWireRemote: {e}"))?;
        let fd: zbus::zvariant::OwnedFd = r.body().deserialize().map_err(|e| e.to_string())?;
        self.fd = Some(fd.into());
        Ok(())
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1)
    }

    pub fn close(&mut self) {
        self.fd = None;
        if let Some(s) = self.session.take() {
            let _ = self.bus.call_method(Some(BUS), &s, Some("org.freedesktop.portal.Session"), "Close", &());
        }
    }
}

impl Drop for ScreenCast {
    fn drop(&mut self) {
        self.close();
    }
}

/// A Wayland session (XWayland's $DISPLAY only sees X11 windows, so x11grab would capture black).
pub fn is_wayland(env: &Env) -> bool {
    env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || env.get("XDG_SESSION_TYPE").map(String::as_str) == Some("wayland")
}

/// PipeWire stream -> raw I420 frames of exactly width x height @ fps on stdout. `virt`: the stream is a virtual
/// screen; asking for that size right at the source makes the compositor give the screen that size.
pub fn gst_cmd(fd: i32, node: u32, width: u32, height: u32, fps: u32, virt: Option<(u32, u32)>) -> Vec<String> {
    let caps =
        format!("video/x-raw,format=I420,width={width},height={height},framerate={fps}/1,pixel-aspect-ratio=1/1");
    #[rustfmt::skip]
    let mut v: Vec<String> = vec![
        "gst-launch-1.0".into(), "-q".into(),
        // keepalive: PipeWire only delivers frames on damage; resend the last one so a static screen keeps flowing
        "pipewiresrc".into(), format!("fd={fd}"), format!("path={node}"), "do-timestamp=true".into(),
        "keepalive-time=100".into(), "always-copy=true".into(),
    ];
    if let Some((w, h)) = virt {
        v.extend(["!".into(), format!("video/x-raw,width={w},height={h}")]);
    }
    #[rustfmt::skip]
    v.extend([
        "!".into(), "videoconvert".into(), "!".into(), "videoscale".into(), "add-borders=true".into(), "!".into(),
        "videorate".into(), "!".into(), caps, "!".into(), "fdsink".into(), "fd=1".into(), "sync=false".into(),
    ]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> Env {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_gst_delivers_exact_negotiated_frames() {
        let cmd = gst_cmd(7, 42, 1920, 1080, 30, None);
        assert_eq!(cmd[..4], ["gst-launch-1.0", "-q", "pipewiresrc", "fd=7"]);
        assert!(cmd.iter().any(|a| a == "path=42"));
        let caps = "video/x-raw,format=I420,width=1920,height=1080,framerate=30/1,pixel-aspect-ratio=1/1";
        assert!(cmd.iter().any(|a| a == caps));
        assert_eq!(cmd[cmd.len() - 3..], ["fdsink", "fd=1", "sync=false"]);
        assert!(!cmd.iter().any(|a| a == "video/x-raw,width=1920,height=1080"));
    }

    #[test]
    fn virtual_screen() {
        let cmd = gst_cmd(7, 42, 1920, 1080, 60, Some((1920, 1080)));
        let at = cmd.iter().position(|a| a == "video/x-raw,width=1920,height=1080").expect("size caps");
        assert_eq!(cmd[at - 1], "!");
        assert!(cmd[at - 2].starts_with("always-copy"), "right after pipewiresrc");
        assert_eq!(cmd[cmd.len() - 3..], ["fdsink", "fd=1", "sync=false"]);
        assert_eq!(virtual_size("1080p30,720p60"), (1920, 1080));
        assert_eq!(virtual_size(" 720p60,1080p30"), (1280, 720));
        assert_eq!(virtual_size(""), (1920, 1080));
        assert_eq!(source_type(true, 1 | 2 | 4), 4);
        assert_eq!(source_type(true, 1 | 2), 1);
        assert_eq!(source_type(false, 7), 1);
    }

    #[test]
    fn token_file_follows_xdg_config_home() {
        let e = env_of(&[("XDG_CONFIG_HOME", "/c"), ("HOME", "/h")]);
        assert_eq!(token_file(&e, false), PathBuf::from("/c/rokast/portal-token"));
        assert_eq!(token_file(&e, true), PathBuf::from("/c/rokast/portal-token-virtual"));
        assert_eq!(token_file(&env_of(&[("HOME", "/h")]), false), PathBuf::from("/h/.config/rokast/portal-token"));
        assert!(token().starts_with("rokast") && token().len() == 14);
    }

    #[test]
    fn test_session_detection() {
        assert!(is_wayland(&env_of(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]))); // XWayland present
        assert!(is_wayland(&env_of(&[("XDG_SESSION_TYPE", "wayland")])));
        assert!(!is_wayland(&env_of(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")])));
    }
}
