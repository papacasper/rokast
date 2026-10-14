//! Screen capture -> H.264(+audio) -> RTP/MPEG-TS, parameterised by the negotiated WFD format
//!: monitor parsing, the encoder probe, the ffmpeg command line, starting it.
use crate::proc;
use crate::pyfmt::repr;
use crate::wfd::Negotiated;
use crate::Env;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

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

/// The default sink's monitor, the sound to send (None without PulseAudio/PipeWire-pulse).
pub fn default_monitor() -> Option<String> {
    let o = proc::run(&["pactl", "get-default-sink"], &[], Duration::from_secs(10)).filter(proc::Output::ok)?;
    let sink = o.stdout.trim();
    (!sink.is_empty()).then(|| format!("{sink}.monitor"))
}

fn home(env: &Env) -> String {
    env.get("HOME").cloned().unwrap_or_default()
}

/// `xrandr --listmonitors` output for `display` ("" if xrandr is missing or fails).
pub fn x11_monitors(display: &str, env: &Env) -> String {
    let xauth =
        env.get("XAUTHORITY").filter(|v| !v.is_empty()).cloned().unwrap_or(format!("{}/.Xauthority", home(env)));
    let envs = [("DISPLAY", display), ("XAUTHORITY", xauth.as_str())];
    proc::run(&["xrandr", "--listmonitors"], &envs, Duration::from_secs(10)).map(|o| o.stdout).unwrap_or_default()
}

/// `wanted` itself, or for "auto" the first encoder that actually encodes here.
pub fn pick_encoder(wanted: &str) -> Result<String, String> {
    if wanted != "auto" {
        return Ok(wanted.into());
    }
    for enc in ENCODERS {
        let args = probe_args(enc);
        let mut argv = vec!["ffmpeg", "-hide_banner", "-loglevel", "error"];
        argv.extend(args.iter().map(String::as_str));
        argv.extend(["-f", "null", "-"]);
        if proc::run(&argv, &[], Duration::from_secs(30)).is_some_and(|o| o.ok()) {
            return Ok(enc.into());
        }
    }
    Err("no working H.264 encoder (install ffmpeg with libx264; Fedora: ffmpeg from RPM Fusion)".into())
}

/// ffmpeg, optionally fed by a GStreamer PipeWire capture (the Wayland path).
pub struct Pipeline {
    pub procs: Vec<Child>,
}

impl Pipeline {
    /// ffmpeg, the last process.
    pub fn ffmpeg(&mut self) -> &mut Child {
        self.procs.last_mut().expect("a pipeline has ffmpeg")
    }

    /// SIGTERM each, ffmpeg first, then SIGKILL what is left after `grace`.
    pub fn terminate(&mut self, grace: Duration) {
        for p in self.procs.iter_mut().rev() {
            proc::terminate(p, grace);
        }
    }
}

/// Start ffmpeg; with `feeder` (gst argv and the PipeWire fd it reads) gst's stdout becomes ffmpeg's stdin.
pub fn start(cmd: &[String], display: &str, env: &Env, feeder: Option<(&[String], i32)>) -> std::io::Result<Pipeline> {
    let xauth = format!("{}/.Xauthority", home(env)); // the session's own cookie file
    let mut ff = Command::new(&cmd[0]);
    ff.args(&cmd[1..]).env("DISPLAY", display).env("XAUTHORITY", xauth);
    let Some((gst_argv, fd)) = feeder else { return Ok(Pipeline { procs: vec![ff.spawn()?] }) };
    let mut gst = Command::new(&gst_argv[0]);
    gst.args(&gst_argv[1..]).stdout(Stdio::piped());
    // SAFETY: only fcntl between fork and exec (async-signal-safe): keep the portal's fd open in gst (pass_fds).
    unsafe {
        gst.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut g = gst.spawn()?;
    let out = g.stdout.take().expect("piped"); // ffmpeg owns the read end; gst gets SIGPIPE if ffmpeg dies
    match ff.stdin(Stdio::from(out)).spawn() {
        Ok(f) => Ok(Pipeline { procs: vec![g, f] }),
        Err(e) => {
            proc::terminate(&mut g, Duration::from_secs(1));
            Err(e)
        }
    }
}

/// Where the frames come from and where the stream goes.
#[derive(Debug, Clone)]
pub struct Opts {
    pub display: String,
    pub encoder: String,
    pub bitrate: String,
    pub audio: bool,
    pub monitor: Option<String>, // pulse source for the sound
    pub audio_delay_ms: i32,     // AUDIO_DELAY: negative plays the sound earlier
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
            audio_delay_ms: 0,
            source: "x11".into(),
            out: None,
            seconds: None,
            geometry: None,
        }
    }
}

/// FPS: "tv" (empty too: the negotiated rate) -> None, or "60" -> Some(60).
pub fn parse_fps(value: &str) -> Result<Option<u32>, String> {
    match value.trim() {
        "" | "tv" => Ok(None),
        "60" => Ok(Some(60)),
        other => Err(format!("FPS must be tv or 60 (got {})", crate::pyfmt::repr(other))),
    }
}

/// FPS=60: encode at `fps` whatever was negotiated (the TVs offer HD at 30 at most, but play 60), with the H.264
/// level raised to what the size needs at that rate (macroblocks per second, H.264 table A-1): 1080p60 4.2, 720p60 3.2.
/// nvenc refuses a level that is too low. Unchanged when the negotiated rate is already that high.
pub fn smooth(n: &Negotiated, fps: u32) -> Negotiated {
    if n.fps >= fps {
        return n.clone();
    }
    const LEVELS: [(&str, u64); 5] =
        [("3.1", 108_000), ("3.2", 216_000), ("4.0", 245_760), ("4.1", 245_760), ("4.2", 522_240)];
    let mbps = u64::from(n.width.div_ceil(16) * n.height.div_ceil(16)) * u64::from(fps);
    let level = LEVELS.iter().find(|(l, max)| *l >= n.level.as_str() && *max >= mbps).map_or("4.2", |(l, _)| l);
    Negotiated { fps, level: level.into(), ..n.clone() }
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
        add(&["-thread_queue_size", "512"]);
        if o.audio_delay_ms != 0 {
            add(&["-itsoffset", &format!("{:.3}", f64::from(o.audio_delay_ms) / 1000.0)]);
        }
        add(&["-f", "pulse", "-i", m]);
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
#[path = "capture_tests.rs"]
mod tests;
