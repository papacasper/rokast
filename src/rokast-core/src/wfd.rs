//! Wi-Fi Display (Miracast) negotiation, the pure parts of src/miracast/wfd.py: format parsing, the format choice,
//! RTSP message parsing. The RTSP source itself (M1..M7, keepalive) comes in phase R3.
use crate::pyfmt::repr;
use std::collections::BTreeMap;
use std::fmt;

pub const CRLF: &str = "\r\n";
pub const PUBLIC: &str = "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER, SETUP, PLAY, PAUSE, TEARDOWN";
pub const DEFAULT_PREF: [&str; 4] = ["1080p30", "720p60", "720p30", "480p60"];

pub type Res = (u32, u32, u32);

/// CEA resolution bit -> (w, h, fps)   (Wi-Fi Display spec table 5-10)
pub fn cea(bit: u32) -> Option<Res> {
    Some(match bit {
        0 => (640, 480, 60),
        1 => (720, 480, 60),
        3 => (720, 576, 50),
        5 => (1280, 720, 30),
        6 => (1280, 720, 60),
        7 => (1920, 1080, 30),
        8 => (1920, 1080, 60),
        10 => (1280, 720, 25),
        11 => (1280, 720, 50),
        12 => (1920, 1080, 25),
        13 => (1920, 1080, 50),
        15 => (1280, 720, 24),
        16 => (1920, 1080, 24),
        _ => return None,
    })
}

pub fn pref_bit(p: &str) -> Option<u32> {
    Some(match p {
        "1080p60" => 8,
        "1080p30" => 7,
        "720p60" => 6,
        "720p30" => 5,
        "480p60" => 0,
        _ => return None,
    })
}

/// 0x04 = L4.0 is enough for everything but 1080p at 50/60 (0x10 = L4.2).
fn level_for(res: Res) -> u32 {
    match res {
        (1920, 1080, 60) | (1920, 1080, 50) => 0x10,
        _ => 0x04,
    }
}

pub fn level_name(bit: u32) -> Option<&'static str> {
    Some(match bit {
        0x01 => "3.1",
        0x02 => "3.2",
        0x04 => "4.0",
        0x08 => "4.1",
        0x10 => "4.2",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Negotiated {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub profile: String, // "baseline" | "high"
    pub level: String,   // e.g. "4.0"
    pub audio: String,   // "AAC" | "LPCM"
    pub rtp_port: u16,
    pub video_param: String, // exact wfd_video_formats string we sent in M4
}

impl Negotiated {
    pub fn new(width: u32, height: u32, fps: u32, profile: &str, level: &str, audio: &str, rtp_port: u16) -> Self {
        Negotiated {
            width,
            height,
            fps,
            profile: profile.into(),
            level: level.into(),
            audio: audio.into(),
            rtp_port,
            video_param: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Caps {
    pub profile: u32,
    pub level: u32,
    pub cea: u32,
    pub vesa: u32,
    pub hh: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Parse the sink's wfd_video_formats value into masks.
pub fn parse_video_formats(s: &str) -> Result<Caps, Error> {
    let bad = || Error(format!("unparseable wfd_video_formats: {}", repr(s)));
    let t: Vec<&str> = s.split_whitespace().collect();
    if t.len() < 11 || t[0] == "none" {
        return Err(bad());
    }
    let h = |i: usize| u32::from_str_radix(t[i], 16).map_err(|_| bad());
    Ok(Caps { profile: h(2)?, level: h(3)?, cea: h(4)?, vesa: h(5)?, hh: h(6)? })
}

/// Pick (bit, (w,h,fps), profile_bit, level_bit) from sink caps.
pub fn choose_video<S: AsRef<str>>(caps: &Caps, prefs: &[S]) -> Result<(u32, Res, u32, u32), Error> {
    let mut order: Vec<u32> = prefs.iter().filter_map(|p| pref_bit(p.as_ref())).collect();
    if prefs.is_empty() {
        order = DEFAULT_PREF.iter().filter_map(|p| pref_bit(p)).collect();
    }
    order.extend([8, 7, 6, 5, 1, 0]);
    for bit in order {
        if caps.cea & (1 << bit) != 0 {
            let res = cea(bit).expect("every bit in the order is in the CEA table");
            let need = level_for(res);
            let lv = caps.level;
            let level = if lv >= need {
                need
            } else if lv != 0 {
                lv & lv.wrapping_neg() // lowest level the sink offers: never exceed what it said
            } else {
                0x01
            };
            // constrained baseline if offered (most compatible), else high
            let prof = if caps.profile & 0x01 != 0 { 0x01 } else { 0x02 };
            return Ok((bit, res, prof, level));
        }
    }
    Err(Error("sink offers no CEA resolution we can use".into()))
}

pub fn format_video_param(bit: u32, prof: u32, level: u32) -> String {
    format!("00 00 {prof:02x} {level:02x} {:08x} 00000000 00000000 00 0000 0000 00 none none", 1u32 << bit)
}

/// One RTSP message: start line, lower-cased headers, body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Msg {
    pub start: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl Msg {
    pub fn is_request(&self) -> bool {
        !self.start.starts_with("RTSP/")
    }

    pub fn method(&self) -> &str {
        self.start.split_whitespace().next().unwrap_or("")
    }

    pub fn cseq(&self) -> i64 {
        self.headers.get("cseq").map(String::as_str).unwrap_or("0").trim().parse().unwrap_or(0)
    }

    /// Parse the head (everything up to and including the blank line); returns the message and its Content-Length.
    pub fn parse_head(head: &[u8]) -> (Msg, usize) {
        let text = String::from_utf8_lossy(head);
        let mut lines = text.split(CRLF);
        let start = lines.next().unwrap_or("").to_string();
        let mut headers = BTreeMap::new();
        for ln in lines {
            if let Some((k, v)) = ln.split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
        let n = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        (Msg { start, headers, body: String::new() }, n)
    }

    /// Read one message from a stream (blocking).
    pub fn read(r: &mut impl std::io::BufRead) -> std::io::Result<Msg> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut line = Vec::new();
            if r.read_until(b'\n', &mut line)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            head.extend(line);
        }
        let (mut m, n) = Msg::parse_head(&head);
        if n > 0 {
            let mut body = vec![0; n];
            r.read_exact(&mut body)?;
            m.body = String::from_utf8_lossy(&body).into_owned();
        }
        Ok(m)
    }
}

/// `key: value` lines of a text/parameters body.
pub fn parse_params(body: &str) -> BTreeMap<String, String> {
    body.lines()
        .filter_map(|ln| ln.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROKU: &str = "00 00 03 10 0001ffff 1fffffff 00000fff 00 0000 0000 11 none none";

    #[test]
    fn test_preference_and_fallback() {
        let caps = parse_video_formats(ROKU).unwrap();
        assert_eq!(choose_video(&caps, &["720p60"]).unwrap().1, (1280, 720, 60));
        let only480 = Caps { cea: 0x1, ..caps };
        assert_eq!(choose_video(&only480, &["1080p30"]).unwrap().1, (640, 480, 60));
    }

    #[test]
    fn test_roku_sink_caps_accepts_unknown_levels() {
        let caps = parse_video_formats("00 00 01 02 00000080 00000000 00000000 00 0000 0000 00 none none").unwrap();
        let (_bit, res, prof, level) = choose_video::<&str>(&caps, &[]).unwrap();
        assert_eq!((res, prof), ((1920, 1080, 30), 1));
        assert_eq!(level, 0x02); // sink only does L3.2 -> never exceed what it said
    }

    #[test]
    fn unparseable_and_unusable() {
        assert_eq!(parse_video_formats("none").unwrap_err().0, "unparseable wfd_video_formats: 'none'");
        let caps = Caps { profile: 1, level: 4, cea: 1 << 2, vesa: 0, hh: 0 };
        assert!(choose_video::<&str>(&caps, &[]).is_err());
    }

    #[test]
    fn video_param_and_levels() {
        assert_eq!(format_video_param(7, 1, 4), "00 00 01 04 00000080 00000000 00000000 00 0000 0000 00 none none");
        let caps = parse_video_formats(ROKU).unwrap();
        assert_eq!(choose_video(&caps, &["1080p60"]).unwrap(), (8, (1920, 1080, 60), 1, 0x10));
        assert_eq!(level_name(0x10), Some("4.2"));
    }

    #[test]
    fn message_parsing() {
        let raw = b"SET_PARAMETER rtsp://x RTSP/1.0\r\nCSeq: 4\r\nContent-Length: 27\r\n\r\nwfd_trigger_method: SETUP\r\nrest";
        let m = Msg::read(&mut &raw[..]).unwrap();
        assert!(m.is_request());
        assert_eq!((m.method(), m.cseq()), ("SET_PARAMETER", 4));
        assert_eq!(parse_params(&m.body)["wfd_trigger_method"], "SETUP");
        let r = Msg::read(&mut &b"RTSP/1.0 200 OK\r\ncseq: 9\r\n\r\n"[..]).unwrap();
        assert!(!r.is_request());
        assert_eq!(r.cseq(), 9);
    }
}
