//! rokast selftest + hardware report: which mode to test, capture + encode to a file,
//! the ffprobe checks, the report helpers.
use crate::capture::{self, Opts};
use crate::conf::{self, Conf};
use crate::pyfmt::{py_str, repr};
use crate::wfd::Negotiated;
use crate::{proc, wayland, Env};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

pub const MODES: [(&str, (u32, u32, u32)); 5] = [
    ("1080p60", (1920, 1080, 60)),
    ("1080p30", (1920, 1080, 30)),
    ("720p60", (1280, 720, 60)),
    ("720p30", (1280, 720, 30)),
    ("480p60", (640, 480, 60)),
];
/// Share of fps * seconds that must arrive (x11grab/portal can drop a few at start).
pub const MIN_FRAMES: f64 = 0.8;

/// What a Roku would most likely negotiate: the first known entry of RES ("1080p30,720p60,..."), High profile,
/// level 4.2 for 1080p60 else 4.0.
pub fn negotiated(res: &str) -> Result<Negotiated, String> {
    for r in res.split(',').map(str::trim) {
        if let Some((_, (w, h, fps))) = MODES.iter().find(|(name, _)| *name == r) {
            let level = if (*h, *fps) == (1080, 60) { "4.2" } else { "4.0" };
            return Ok(Negotiated::new(*w, *h, *fps, "high", level, "AAC", 0));
        }
    }
    let known: Vec<&str> = MODES.iter().map(|(n, _)| *n).collect();
    Err(format!("RES has no known mode (got {}; known: {})", repr(res), known.join(", ")))
}

pub fn probe_cmd(path: &str) -> Vec<String> {
    ["ffprobe", "-v", "error", "-count_frames", "-show_entries"]
        .into_iter()
        .chain(["stream=codec_type,codec_name,width,height,nb_read_frames", "-of", "json", path])
        .map(String::from)
        .collect()
}

fn streams(probe: &Value) -> &[Value] {
    probe.get("streams").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn of_type(s: &Value, t: &str) -> bool {
    s.get("codec_type").and_then(Value::as_str) == Some(t)
}

/// ffprobe JSON + expectations -> list of problems (empty = pass).
pub fn check(probe: &Value, n: &Negotiated, seconds: u32, audio: bool) -> Vec<String> {
    let st = streams(probe);
    let Some(v) = st.iter().find(|s| of_type(s, "video")) else {
        return vec!["no video stream in the output".into()];
    };
    let mut problems = Vec::new();
    let g = |k: &str| py_str(v.get(k));
    if v.get("codec_name").and_then(Value::as_str) != Some("h264") {
        problems.push(format!("video codec {}, expected h264", g("codec_name")));
    }
    let dim = |k: &str| v.get(k).and_then(Value::as_u64);
    if (dim("width"), dim("height")) != (Some(n.width.into()), Some(n.height.into())) {
        problems.push(format!("video {}x{}, expected {}x{}", g("width"), g("height"), n.width, n.height));
    }
    let frames = match v.get("nb_read_frames") {
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        Some(Value::Number(x)) => x.as_i64().unwrap_or(0),
        _ => 0,
    };
    let want = (f64::from(n.fps * seconds) * MIN_FRAMES) as i64;
    if frames < want {
        problems.push(format!("only {frames} frames in {seconds}s, expected at least {want} ({} fps)", n.fps));
    }
    if audio && !st.iter().any(|s| of_type(s, "audio")) {
        problems
            .push("no audio stream (AUDIO=1; is PulseAudio/PipeWire-pulse running? `pactl get-default-sink`)".into());
    }
    problems
}

pub fn summary(probe: &Value) -> String {
    let mut parts = Vec::new();
    for s in streams(probe) {
        let g = |k: &str| py_str(s.get(k));
        if of_type(s, "video") {
            parts.push(format!("{} {}x{} {} frames", g("codec_name"), g("width"), g("height"), g("nb_read_frames")));
        } else if of_type(s, "audio") {
            parts.push(format!("{} audio", g("codec_name")));
        }
    }
    parts.join(", ")
}

pub fn os_release(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return "unknown".into();
    };
    text.lines()
        .find_map(|ln| ln.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// [(iface, driver)] for wireless interfaces.
pub fn wifi_chips(sys_net: &Path) -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(sys_net) else {
        return vec![];
    };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
        .into_iter()
        .filter(|n| sys_net.join(n).join("wireless").is_dir())
        .map(|n| {
            let drv = sys_net.join(&n).join("device").join("driver");
            let name =
                fs::canonicalize(&drv).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            (n, name.unwrap_or_else(|| "?".into()))
        })
        .collect()
}

/// Capture + encode `seconds` into `out` (a temp file, deleted after, if None). Ok(true) on pass.
pub fn run(
    conf: &Conf,
    mode: &str,
    seconds: u32,
    out: Option<&str>,
    env: &Env,
    log: &dyn Fn(&str),
) -> Result<bool, String> {
    if proc::which("ffprobe", env).is_none() {
        return Err("ffprobe not found (it comes with ffmpeg)".into());
    }
    let n = negotiated(&conf["RES"])?;
    let encoder = capture::pick_encoder(&conf["ENCODER"])?;
    let monitor = crate::audio::plain_source(&conf["AUDIO"], env)?;
    let tmp = match out {
        Some(_) => None,
        None => Some(
            tempfile::Builder::new()
                .prefix("rokast-selftest-")
                .suffix(".ts")
                .tempfile()
                .map_err(|e| e.to_string())?
                .into_temp_path(),
        ),
    };
    let out = out
        .map(String::from)
        .unwrap_or_else(|| tmp.as_ref().map(|t| t.to_string_lossy().into_owned()).unwrap_or_default());
    let of = if conf["MONITOR"].is_empty() { String::new() } else { format!(" of {}", conf["MONITOR"]) };
    let sound = monitor.as_ref().map(|m| format!("from {m}")).unwrap_or("off".into());
    log(&format!(
        "selftest: {mode} capture{of}, {encoder}, {}x{}@{}, audio {sound}, {seconds}s -> {out}",
        n.width, n.height, n.fps
    ));
    let mut cast = None;
    if mode == "wayland" {
        let mut c = wayland::ScreenCast::new()?;
        let want = (conf["SCREEN"] == "extend").then(|| wayland::virtual_size(&conf["RES"]));
        c.open(env, want).map_err(|e| match e {
            crate::stop::Fail::Err(m) => m,
            crate::stop::Fail::Stop => "stopped".into(),
        })?;
        if want.is_some() {
            log(&wayland::extend_note(c.virt));
        }
        cast = Some(c);
    }
    let geometry = match cast {
        Some(_) => None,
        None => capture::monitor_geometry(&capture::x11_monitors(&conf["DISPLAY"], env), &conf["MONITOR"])?,
    };
    let opts = Opts {
        display: conf["DISPLAY"].clone(),
        encoder: encoder.clone(),
        bitrate: conf["BITRATE"].clone(),
        audio: monitor.is_some(),
        monitor: monitor.clone(),
        audio_delay_ms: conf::audio_delay(&conf["AUDIO_DELAY"])?,
        source: if cast.is_some() { "raw" } else { "x11" }.into(),
        out: Some(out.clone()),
        seconds: Some(seconds),
        geometry,
    };
    let cmd = capture::build_cmd(&n, None, None, &opts);
    let feeder =
        cast.as_ref().map(|c| (wayland::gst_cmd(c.raw_fd(), c.node, n.width, n.height, n.fps, c.virt), c.raw_fd()));
    let feeder = feeder.as_ref().map(|(argv, fd)| (argv.as_slice(), *fd));
    let mut pipe = capture::start(&cmd, &conf["DISPLAY"], env, feeder).map_err(|e| format!("ffmpeg: {e}"))?;
    let code = match proc::wait_for(pipe.ffmpeg(), Duration::from_secs(u64::from(seconds) + 30)) {
        Some(st) => proc::returncode(&st).to_string(),
        None => "timeout".into(),
    };
    pipe.terminate(Duration::from_secs(5));
    drop(cast); // closes the portal session
    if code != "0" {
        log(&format!("selftest: FAIL ffmpeg exited with {code} (its messages are above)"));
        if encoder == "h264_vaapi" {
            log("selftest: hint: VAAPI needs your GPU's VA-API driver (intel-media-driver for recent Intel, \
                 Mesa VA-API for AMD); ENCODER=libx264 works everywhere");
        }
        return Ok(false);
    }
    let pc = probe_cmd(&out);
    let argv: Vec<&str> = pc.iter().map(String::as_str).collect();
    let r = proc::run(&argv, &[], Duration::from_secs(60)).ok_or("ffprobe did not finish")?;
    if !r.ok() {
        log(&format!("selftest: FAIL ffprobe could not read the output: {}", r.stderr.trim()));
        return Ok(false);
    }
    let probe: Value = serde_json::from_str(&r.stdout).map_err(|e| e.to_string())?;
    let problems = check(&probe, &n, seconds, monitor.is_some());
    for p in &problems {
        log(&format!("selftest: FAIL {p}"));
    }
    if problems.is_empty() {
        log(&format!("selftest: ok {}", summary(&probe)));
    }
    Ok(problems.is_empty())
}

fn first_line(argv: &[&str]) -> Option<String> {
    proc::run(argv, &[], Duration::from_secs(10)).and_then(|o| o.stdout.lines().next().map(String::from))
}

/// [(label, value)] for a hardware report.
pub fn system_info(env: &Env) -> Vec<(String, String)> {
    let pci = proc::run(&["lspci"], &[], Duration::from_secs(10)).map(|o| o.stdout).unwrap_or_default();
    let after = |ln: &str| ln.split_once(": ").map(|(_, r)| r).unwrap_or(ln).to_string();
    let pick = |keys: &[&str]| -> String {
        let v: Vec<String> = pci.lines().filter(|l| keys.iter().any(|k| l.contains(k))).map(after).collect();
        v.join("; ")
    };
    let gpus = pick(&["VGA", "3D controller", "Display controller"]);
    let nets = pick(&["Network controller"]);
    let chips: Vec<String> = wifi_chips(Path::new("/sys/class/net")).iter().map(|(i, d)| format!("{i}: {d}")).collect();
    let or = |s: String, d: &str| if s.is_empty() { d.to_string() } else { s };
    let var = |k: &str| env.get(k).filter(|v| !v.is_empty()).cloned();
    let session = var("XDG_SESSION_TYPE").unwrap_or(if wayland::is_wayland(env) { "wayland" } else { "x11?" }.into());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()).unwrap_or_default();
    [
        ("distro", os_release(Path::new("/etc/os-release"))),
        ("kernel", kernel),
        ("desktop", var("XDG_CURRENT_DESKTOP").unwrap_or("unknown".into())),
        ("session", session),
        ("gpu", or(gpus, "unknown (lspci missing?)")),
        ("wifi", or(nets, "unknown")),
        ("wifi driver", or(chips.join(", "), "none")),
        ("ffmpeg", first_line(&["ffmpeg", "-version"]).unwrap_or("missing".into())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe(frames: u32, w: u32, h: u32, codec: &str, audio: bool) -> Value {
        let mut s = vec![json!({"codec_type": "video", "codec_name": codec, "width": w, "height": h,
                                "nb_read_frames": frames.to_string()})];
        if audio {
            s.push(json!({"codec_type": "audio", "codec_name": "aac"}));
        }
        json!({ "streams": s })
    }

    fn good() -> Value {
        probe(150, 1920, 1080, "h264", true)
    }

    #[test]
    fn test_negotiated_takes_first_known_mode() {
        let n = negotiated("4k, 720p60,1080p30").unwrap();
        assert_eq!((n.width, n.height, n.fps, n.profile.as_str()), (1280, 720, 60, "high"));
        assert_eq!(negotiated("1080p60").unwrap().level, "4.2");
        assert!(negotiated("4k").is_err());
    }

    #[test]
    fn test_good_file_passes() {
        let n = negotiated("1080p30").unwrap();
        assert!(check(&good(), &n, 5, true).is_empty());
        assert!(check(&probe(120, 1920, 1080, "h264", true), &n, 5, true).is_empty());
        // 80% of 150
    }

    #[test]
    fn test_each_problem_is_reported() {
        let n = negotiated("1080p30").unwrap();
        assert_eq!(check(&json!({"streams": []}), &n, 5, false), ["no video stream in the output"]);
        assert!(check(&probe(119, 1920, 1080, "h264", true), &n, 5, true)[0].contains("frames"));
        assert!(check(&probe(150, 1280, 720, "h264", true), &n, 5, true)[0].contains("1280x720"));
        assert!(check(&probe(150, 1920, 1080, "hevc", true), &n, 5, true)[0].contains("hevc"));
        assert!(check(&probe(150, 1920, 1080, "h264", false), &n, 5, true)[0].contains("audio"));
        assert!(check(&probe(150, 1920, 1080, "h264", false), &n, 5, false).is_empty());
    }

    #[test]
    fn test_summary() {
        assert_eq!(summary(&good()), "h264 1920x1080 150 frames, aac audio");
    }

    #[test]
    fn test_os_release() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("os-release");
        fs::write(&p, "NAME=\"X\"\nPRETTY_NAME=\"Manjaro Linux\"\n").unwrap();
        assert_eq!(os_release(&p), "Manjaro Linux");
        assert_eq!(os_release(&t.path().join("nope")), "unknown");
    }

    #[test]
    fn test_wifi_chips_finds_wireless_ifaces_and_driver() {
        let t = tempfile::tempdir().unwrap();
        let drv = t.path().join("drivers/iwlwifi");
        fs::create_dir_all(&drv).unwrap();
        fs::create_dir_all(t.path().join("net/wlo1/wireless")).unwrap();
        fs::create_dir_all(t.path().join("net/wlo1/device")).unwrap();
        std::os::unix::fs::symlink(&drv, t.path().join("net/wlo1/device/driver")).unwrap();
        fs::create_dir_all(t.path().join("net/eth0")).unwrap();
        assert_eq!(wifi_chips(&t.path().join("net")), [("wlo1".to_string(), "iwlwifi".to_string())]);
    }
}
