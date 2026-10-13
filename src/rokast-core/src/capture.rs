//! Screen capture -> H.264(+audio) -> RTP/MPEG-TS, parameterised by the negotiated WFD format
//! (src/miracast/capture.py). Pure parts: monitor parsing and the ffmpeg command line. Starting the pipeline: R3.
use crate::pyfmt::repr;
use crate::wfd::Negotiated;

/// Auto-detect order: NVIDIA, Intel/AMD (VAAPI), software; libopenh264 is the software encoder Fedora's
/// ffmpeg-free has.
pub const ENCODERS: [&str; 4] = ["h264_nvenc", "h264_vaapi", "libx264", "libopenh264"];
pub const BETTER_ENCODER: &str = "full ffmpeg (Fedora: RPM Fusion's ffmpeg) has libx264, sharper at the same bitrate";
pub const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

/// One `xrandr --listmonitors` line: ` 1: +*HDMI-1 2560/597x1440/336+1920+0  HDMI-1`.
fn parse_monitor_line(ln: &str) -> Option<Monitor> {
    let rest = ln.trim_start();
    let (idx, rest) = rest.split_once(':')?;
    if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let flags_len = rest.find(|c| c != '+' && c != '*').unwrap_or(rest.len());
    let (flags, rest) = rest.split_at(flags_len);
    let (name, rest) = rest.split_once(char::is_whitespace)?;
    let geo = rest.split_whitespace().next()?;
    let num =
        |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<i64>().ok()).flatten();
    // W/mmW x H/mmH + X + Y
    let (w, rest) = geo.split_once('/')?;
    let (_mmw, rest) = rest.split_once('x')?;
    let (h, rest) = rest.split_once('/')?;
    let mut parts = rest.splitn(3, '+');
    let _mmh = num(parts.next()?)?;
    let x = parts.next()?;
    let y_full = parts.next()?;
    let y_len = y_full.find(|c: char| !c.is_ascii_digit()).unwrap_or(y_full.len());
    Some(Monitor {
        name: name.to_string(),
        width: num(w)? as u32,
        height: num(h)? as u32,
        x: num(x)? as i32,
        y: num(&y_full[..y_len])? as i32,
        primary: flags.contains('*'),
    })
}

/// `xrandr --listmonitors` -> monitors.
pub fn parse_monitors(xrandr_output: &str) -> Vec<Monitor> {
    xrandr_output.lines().filter_map(parse_monitor_line).collect()
}

/// The monitor to capture. `name` empty = the primary one (else the first). None when xrandr listed no monitors
/// (capture the whole X screen). Unknown name -> error naming the choices.
pub fn monitor_geometry(xrandr_output: &str, name: &str) -> Result<Option<Monitor>, String> {
    let mons = parse_monitors(xrandr_output);
    if mons.is_empty() {
        return Ok(None);
    }
    if name.is_empty() {
        return Ok(Some(mons.iter().find(|m| m.primary).unwrap_or(&mons[0]).clone()));
    }
    if let Some(m) = mons.iter().find(|m| m.name == name) {
        return Ok(Some(m.clone()));
    }
    let names: Vec<&str> = mons.iter().map(|m| m.name.as_str()).collect();
    Err(format!("MONITOR={} not found; monitors: {}", repr(name), names.join(", ")))
}

/// ffmpeg arguments that test whether `encoder` really encodes here (being listed doesn't mean the GPU is there).
pub fn probe_args(encoder: &str) -> Vec<String> {
    let mut a: Vec<&str> = Vec::new();
    if encoder == "h264_vaapi" {
        a.extend(["-vaapi_device", VAAPI_DEVICE]);
    }
    a.extend(["-f", "lavfi", "-i", "testsrc2=s=640x360:d=0.2"]);
    if encoder == "h264_vaapi" {
        a.extend(["-vf", "format=nv12,hwupload"]);
    }
    a.extend(["-c:v", encoder]);
    a.into_iter().map(String::from).collect()
}

/// Where the frames come from and where the stream goes.
#[derive(Debug, Clone)]
pub struct Opts {
    pub display: String,
    pub encoder: String,
    pub bitrate: String,
    pub audio: bool,
    pub monitor: Option<String>, // pulse source for the sound
    pub source: String,          // "x11" | "raw" (I420 frames on stdin: the Wayland path)
    pub out: Option<String>,     // MPEG-TS file instead of RTP (selftest)
    pub seconds: Option<u32>,
    pub geometry: Option<Monitor>, // x11grab only that monitor
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            display: ":0".into(),
            encoder: "h264_nvenc".into(),
            bitrate: "8M".into(),
            audio: true,
            monitor: None,
            source: "x11".into(),
            out: None,
            seconds: None,
            geometry: None,
        }
    }
}

fn level_flag(level: &str) -> &str {
    match level {
        "3.1" | "3.2" | "4.0" | "4.1" | "4.2" => level,
        _ => "4.0",
    }
}

/// Negotiated + target -> ffmpeg argv.
pub fn build_cmd(n: &Negotiated, sink_ip: Option<&str>, server_port: Option<u16>, o: &Opts) -> Vec<String> {
    let gop = n.fps.to_string(); // 1s GOP: sink can resync/IDR-request cheaply
    let display = o.display.split('.').next().unwrap_or(""); // ":0.0" (inherited $DISPLAY) -> ":0"
    let vaapi = o.encoder == "h264_vaapi";
    let mut cmd: Vec<String> = Vec::new();
    let mut add = |items: &[&str]| cmd.extend(items.iter().map(|s| s.to_string()));
    add(&["ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin", "-fflags", "nobuffer", "-flags", "low_delay"]);
    if vaapi {
        add(&["-vaapi_device", VAAPI_DEVICE]);
    }
    let (w, h, fps) = (n.width, n.height, n.fps.to_string());
    if o.source == "raw" {
        add(&["-f", "rawvideo", "-pix_fmt", "yuv420p", "-video_size", &format!("{w}x{h}")]);
        add(&["-framerate", &fps, "-i", "pipe:0"]);
    } else {
        add(&["-f", "x11grab", "-framerate", &fps, "-draw_mouse", "1"]);
        let (x, y) = o.geometry.as_ref().map(|g| (g.x, g.y)).unwrap_or((0, 0));
        if let Some(g) = &o.geometry {
            add(&["-video_size", &format!("{}x{}", g.width, g.height)]);
        }
        add(&["-i", &format!("{display}.0+{x},{y}")]);
    }
    let monitor = o.monitor.as_deref().filter(|m| o.audio && !m.is_empty());
    if let Some(m) = monitor {
        add(&["-thread_queue_size", "512", "-f", "pulse", "-i", m]);
    }
    let fmt = if vaapi { "format=nv12,hwupload" } else { "format=yuv420p" };
    let vf = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,{fmt}");
    add(&["-map", "0:v", "-vf", &vf]);
    let b = o.bitrate.as_str();
    match o.encoder.as_str() {
        "h264_nvenc" => add(&[
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ll",
            "-rc",
            "cbr",
            "-b:v",
            b,
            "-maxrate",
            b,
            "-bufsize",
            "1M",
            "-bf",
            "0",
            "-rc-lookahead",
            "0",
            "-forced-idr",
            "1",
        ]),
        "h264_vaapi" => add(&["-c:v", "h264_vaapi", "-rc_mode", "CBR", "-b:v", b, "-maxrate", b, "-bf", "0"]),
        // no presets/tune; no B-frames by design; skipping frames would stall the TV
        "libopenh264" => add(&["-c:v", "libopenh264", "-b:v", b, "-maxrate", b, "-allow_skip_frames", "0"]),
        _ => add(&[
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-b:v",
            b,
            "-maxrate",
            b,
            "-bufsize",
            "1M",
            "-bf",
            "0",
        ]),
    }
    let level = level_flag(&n.level);
    let baseline = n.profile == "baseline";
    if vaapi {
        // its names differ: "constrained_baseline", and level as level_idc ("4.0" breaks encoding)
        let idc = (level.parse::<f64>().unwrap_or(4.0) * 10.0).round() as u32;
        add(&["-profile:v", if baseline { "constrained_baseline" } else { "high" }, "-level", &idc.to_string()]);
    } else if o.encoder == "libopenh264" {
        // constrained baseline only (a High decoder plays it); its -level is an int, so leave the level to it
        add(&["-profile:v", "constrained_baseline"]);
    } else {
        add(&["-profile:v", if baseline { "baseline" } else { "high" }, "-level", level]);
    }
    add(&["-g", &gop, "-keyint_min", &gop]);
    if monitor.is_some() {
        add(&["-map", "1:a"]);
        if n.audio == "AAC" {
            add(&["-c:a", "aac", "-b:a", "160k"]);
        } else {
            add(&["-c:a", "pcm_s16be"]);
        }
        add(&["-ar", "48000", "-ac", "2", "-af", "aresample=async=1:first_pts=0"]);
    }
    add(&["-muxdelay", "0", "-muxpreload", "0", "-flush_packets", "1"]);
    add(&["-mpegts_flags", "+resend_headers", "-pat_period", "0.2"]);
    if let Some(out) = o.out.as_deref().filter(|s| !s.is_empty()) {
        if let Some(s) = o.seconds.filter(|s| *s > 0) {
            add(&["-t", &s.to_string()]);
        }
        add(&["-y", "-f", "mpegts", out]);
    } else {
        let ip = sink_ip.unwrap_or("None");
        let lp = server_port.map(|p| p.to_string()).unwrap_or_else(|| "None".into());
        add(&["-f", "rtp_mpegts", &format!("rtp://{ip}:{}?localport={lp}&pkt_size=1316", n.rtp_port)]);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neg(w: u32, h: u32, fps: u32, profile: &str) -> Negotiated {
        Negotiated::new(w, h, fps, profile, "4.0", "AAC", 19000)
    }

    fn at<'a>(cmd: &'a [String], flag: &str) -> &'a str {
        &cmd[cmd.iter().position(|a| a == flag).unwrap() + 1]
    }

    fn idx(cmd: &[String], flag: &str) -> usize {
        cmd.iter().position(|a| a == flag).unwrap()
    }

    fn opts(encoder: &str) -> Opts {
        Opts { encoder: encoder.into(), ..Opts::default() }
    }

    const XRANDR: &str =
        "Monitors: 2\n 0: +*eDP-1 1920/344x1200/215+0+0  eDP-1\n 1: +HDMI-1 2560/597x1440/336+1920+0  HDMI-1\n";

    #[test]
    fn test_rtp_output_and_profile() {
        let o = Opts { monitor: Some("m.monitor".into()), ..Opts::default() };
        let cmd = build_cmd(&neg(1920, 1080, 30, "baseline"), Some("192.168.49.2"), Some(22222), &o);
        assert_eq!(cmd.last().unwrap(), "rtp://192.168.49.2:19000?localport=22222&pkt_size=1316");
        assert_eq!(at(&cmd, "-profile:v"), "baseline");
        assert!(cmd.iter().any(|a| a == "pulse"));
        assert_eq!(at(&cmd, "-g"), "30");
    }

    #[test]
    fn test_display_with_screen_suffix() {
        let o = Opts { display: ":0.0".into(), ..Opts::default() };
        let cmd = build_cmd(&neg(1920, 1080, 30, "high"), Some("192.168.49.2"), Some(22222), &o);
        assert_eq!(cmd[idx(&cmd, "x11grab") + 6], ":0.0+0,0");
    }

    #[test]
    fn test_vaapi_uploads_frames_to_gpu() {
        let cmd = build_cmd(&neg(1920, 1080, 30, "high"), Some("192.168.49.2"), Some(22222), &opts("h264_vaapi"));
        assert!(idx(&cmd, "-vaapi_device") < idx(&cmd, "-i"));
        assert!(at(&cmd, "-vf").ends_with("format=nv12,hwupload"));
    }

    #[test]
    fn test_vaapi_profile_and_level_spelling() {
        let mut n = neg(1920, 1080, 30, "baseline");
        let cmd = build_cmd(&n, Some("192.168.49.2"), Some(22222), &opts("h264_vaapi"));
        assert_eq!(at(&cmd, "-profile:v"), "constrained_baseline");
        assert_eq!(at(&cmd, "-level"), "40");
        n.level = "4.2".into();
        assert_eq!(at(&build_cmd(&n, Some("x"), Some(1), &opts("h264_vaapi")), "-level"), "42");
    }

    #[test]
    fn test_openh264_for_fedora_ffmpeg_free() {
        let o = Opts { bitrate: "6M".into(), ..opts("libopenh264") };
        let cmd = build_cmd(&neg(1920, 1080, 30, "high"), Some("192.168.49.2"), Some(22222), &o);
        assert_eq!(at(&cmd, "-c:v"), "libopenh264");
        assert_eq!(at(&cmd, "-profile:v"), "constrained_baseline");
        assert!(!cmd.iter().any(|a| a == "-level" || a == "-preset"));
        assert_eq!(at(&cmd, "-b:v"), "6M");
        assert_eq!(ENCODERS[3], "libopenh264"); // last resort: libx264 is better when present
    }

    #[test]
    fn test_file_output_for_selftest() {
        let o = Opts { out: Some("/tmp/x.ts".into()), seconds: Some(5), ..Opts::default() };
        let cmd = build_cmd(&neg(1280, 720, 30, "high"), None, None, &o);
        assert_eq!(&cmd[idx(&cmd, "-t")..], ["-t", "5", "-y", "-f", "mpegts", "/tmp/x.ts"]);
        assert!(!cmd.iter().any(|a| a == "rtp_mpegts"));
    }

    #[test]
    fn test_raw_stdin_input_for_wayland() {
        let o = Opts { source: "raw".into(), ..Opts::default() };
        let cmd = build_cmd(&neg(1280, 720, 60, "high"), Some("192.168.49.2"), Some(22222), &o);
        assert!(!cmd.iter().any(|a| a == "x11grab"));
        let i = idx(&cmd, "rawvideo");
        let want = ["rawvideo", "-pix_fmt", "yuv420p", "-video_size", "1280x720", "-framerate", "60", "-i", "pipe:0"];
        assert_eq!(&cmd[i..i + 9], want);
    }

    #[test]
    fn test_parse() {
        let m = parse_monitors(XRANDR);
        assert_eq!(m.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["eDP-1", "HDMI-1"]);
        let want = Monitor { name: "HDMI-1".into(), width: 2560, height: 1440, x: 1920, y: 0, primary: false };
        assert_eq!(m[1], want);
        assert!(m[0].primary);
    }

    #[test]
    fn test_geometry_choice() {
        let name = |s: &str, n: &str| monitor_geometry(s, n).unwrap().unwrap().name;
        assert_eq!(name(XRANDR, ""), "eDP-1"); // empty -> primary
        assert_eq!(name(&XRANDR.replace("+*", "+"), ""), "eDP-1"); // no primary -> first
        assert_eq!(monitor_geometry(XRANDR, "HDMI-1").unwrap().unwrap().x, 1920);
        assert_eq!(monitor_geometry("", ""), Ok(None)); // xrandr missing: whole screen
        assert!(monitor_geometry(XRANDR, "DP-3").unwrap_err().contains("eDP-1, HDMI-1"));
    }

    #[test]
    fn test_x11grab_captures_only_that_monitor() {
        let g = monitor_geometry(XRANDR, "HDMI-1").unwrap();
        let o = Opts { display: ":0.0".into(), geometry: g, ..Opts::default() };
        let cmd = build_cmd(&neg(1920, 1080, 30, "high"), Some("1.2.3.4"), Some(1), &o);
        let i = idx(&cmd, "-video_size");
        assert_eq!(&cmd[i..i + 4], ["-video_size", "2560x1440", "-i", ":0.0+1920,0"]);
        assert!(at(&cmd, "-vf").contains("scale=1920:1080:force_original_aspect_ratio=decrease"));
    }

    #[test]
    fn probe() {
        assert_eq!(probe_args("libx264").join(" "), "-f lavfi -i testsrc2=s=640x360:d=0.2 -c:v libx264");
        assert_eq!(probe_args("h264_vaapi")[..2], ["-vaapi_device", VAAPI_DEVICE]);
    }
}
