//! Wayland screen capture (src/miracast/wayland.py). Pure parts: session detection and the GStreamer feeder. The
//! portal ScreenCast session over D-Bus comes in phase R4.
use crate::Env;

/// A Wayland session (XWayland's $DISPLAY only sees X11 windows, so x11grab would capture black).
pub fn is_wayland(env: &Env) -> bool {
    env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || env.get("XDG_SESSION_TYPE").map(String::as_str) == Some("wayland")
}

/// PipeWire stream -> raw I420 frames of exactly width x height @ fps on stdout.
pub fn gst_cmd(fd: i32, node: u32, width: u32, height: u32, fps: u32) -> Vec<String> {
    let caps =
        format!("video/x-raw,format=I420,width={width},height={height},framerate={fps}/1,pixel-aspect-ratio=1/1");
    #[rustfmt::skip]
    let v = vec![
        "gst-launch-1.0".into(), "-q".into(),
        // keepalive: PipeWire only delivers frames on damage; resend the last one so a static screen keeps flowing
        "pipewiresrc".into(), format!("fd={fd}"), format!("path={node}"), "do-timestamp=true".into(),
        "keepalive-time=100".into(), "always-copy=true".into(),
        "!".into(), "videoconvert".into(), "!".into(), "videoscale".into(), "add-borders=true".into(), "!".into(),
        "videorate".into(), "!".into(), caps, "!".into(), "fdsink".into(), "fd=1".into(), "sync=false".into(),
    ];
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
        let cmd = gst_cmd(7, 42, 1920, 1080, 30);
        assert_eq!(cmd[..4], ["gst-launch-1.0", "-q", "pipewiresrc", "fd=7"]);
        assert!(cmd.iter().any(|a| a == "path=42"));
        let caps = "video/x-raw,format=I420,width=1920,height=1080,framerate=30/1,pixel-aspect-ratio=1/1";
        assert!(cmd.iter().any(|a| a == caps));
        assert_eq!(cmd[cmd.len() - 3..], ["fdsink", "fd=1", "sync=false"]);
    }

    #[test]
    fn test_session_detection() {
        assert!(is_wayland(&env_of(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]))); // XWayland present
        assert!(is_wayland(&env_of(&[("XDG_SESSION_TYPE", "wayland")])));
        assert!(!is_wayland(&env_of(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")])));
    }
}
