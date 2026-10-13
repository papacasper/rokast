//! rokast.conf: defaults, the KEY=value file, environment overrides.
//! The app reads the same file (ui/src/config.rs).
use crate::pyfmt::{is_printable, repr};
use crate::{wayland, Env};
use std::collections::BTreeMap;

pub type Conf = BTreeMap<String, String>;

pub const DEFAULTS: [(&str, &str); 11] = [
    ("TARGET", ""),
    ("BITRATE", "8M"),
    ("ENCODER", "auto"),
    ("AUDIO", "tv"),
    ("DISPLAY", ":0"),
    ("RES", "1080p30,720p60,720p30"),
    ("NAME", ""),
    ("CAPTURE", "auto"),
    ("MONITOR", ""),
    ("CHECK_UPDATES", "1"),
    ("AUTO_UPDATE", "0"),
];

/// Defaults, then the file's KEY=value lines (`#` starts a comment), then the environment for known keys. NAME is
/// left as written; the caller fills an empty one with default_name().
pub fn parse(text: Option<&str>, env: &Env) -> Conf {
    let mut conf: Conf = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for ln in text.unwrap_or("").lines() {
        let ln = ln.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = ln.split_once('=') {
            conf.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let known: Vec<String> = conf.keys().cloned().collect();
    for k in known {
        if let Some(v) = env.get(&k) {
            conf.insert(k, v.clone());
        }
    }
    let name = clean_name(&conf["NAME"]);
    conf.insert("NAME".into(), name);
    conf
}

/// Python's str.strip() whitespace set.
fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// The name the TV shows: printable, at most 32 bytes (the Wi-Fi Direct device-name limit), never splitting a
/// UTF-8 character.
pub fn clean_name(name: &str) -> String {
    let printable: String = name.chars().filter(|c| is_printable(*c)).collect();
    let mut s = strip(&printable).to_string();
    while s.len() > 32 {
        s.pop();
    }
    strip(&s).to_string()
}

/// "x11" or "wayland" for CAPTURE (auto: from the session).
pub fn capture_mode(conf: &Conf, env: &Env) -> Result<&'static str, String> {
    match conf.get("CAPTURE").map(String::as_str).unwrap_or("auto") {
        "auto" => Ok(if wayland::is_wayland(env) { "wayland" } else { "x11" }),
        "x11" => Ok("x11"),
        "wayland" => Ok("wayland"),
        other => Err(format!("CAPTURE must be auto, x11 or wayland (got {})", repr(other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_name_limits_to_32_bytes() {
        assert_eq!(clean_name("  Casper's Laptop\n"), "Casper's Laptop");
        assert_eq!(clean_name(&"é".repeat(40)).len(), 32); // never splits a UTF-8 character
        assert_eq!(clean_name("\x00\x07"), "");
    }

    #[test]
    fn test_load_conf_reads_file_and_env_beats_file() {
        let env: Env = [("BITRATE".to_string(), "4M".to_string()), ("OTHER".to_string(), "x".to_string())].into();
        let c = parse(Some("TARGET = Living Room # the TV\nBITRATE=6M\n# NAME=x\nnot a line\nNAME= Den PC \n"), &env);
        assert_eq!(c["TARGET"], "Living Room");
        assert_eq!(c["BITRATE"], "4M"); // environment wins
        assert_eq!(c["NAME"], "Den PC");
        assert_eq!(c["RES"], "1080p30,720p60,720p30");
        assert!(!c.contains_key("OTHER"));
    }

    #[test]
    fn capture_modes() {
        let wl: Env = [("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string())].into();
        let mut c = parse(None, &Env::new());
        assert_eq!(capture_mode(&c, &wl), Ok("wayland"));
        assert_eq!(capture_mode(&c, &Env::new()), Ok("x11"));
        c.insert("CAPTURE".into(), "x11".into());
        assert_eq!(capture_mode(&c, &wl), Ok("x11"));
        c.insert("CAPTURE".into(), "vnc".into());
        assert_eq!(capture_mode(&c, &wl), Err("CAPTURE must be auto, x11 or wayland (got 'vnc')".into()));
    }
}
