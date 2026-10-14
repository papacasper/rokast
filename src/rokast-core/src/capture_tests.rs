//! capture.rs tests: the ffmpeg argv, FPS, monitors, pipelines.
use super::*;

#[test]
fn smooth_raises_rate_and_level() {
    let n = Negotiated::new(1920, 1080, 30, "high", "4.0", "AAC", 19000);
    let s = smooth(&n, 60);
    assert_eq!((s.fps, s.level.as_str(), s.width), (60, "4.2", 1920));
    let n = Negotiated::new(1280, 720, 30, "high", "3.1", "AAC", 19000);
    assert_eq!((smooth(&n, 60).fps, smooth(&n, 60).level.as_str()), (60, "3.2"));
    let n = Negotiated::new(1280, 720, 30, "high", "4.0", "AAC", 19000);
    assert_eq!(smooth(&n, 60).level, "4.0", "never lowered");
    let n = Negotiated::new(1280, 720, 60, "high", "3.2", "AAC", 19000);
    assert_eq!(smooth(&n, 60), n);
    assert_eq!(parse_fps(""), Ok(None));
    assert_eq!(parse_fps("tv"), Ok(None));
    assert_eq!(parse_fps(" 60"), Ok(Some(60)));
    assert_eq!(parse_fps("120"), Err("FPS must be tv or 60 (got '120')".into()));
}

fn neg(w: u32, h: u32, fps: u32, profile: &str) -> Negotiated {
    Negotiated::new(w, h, fps, profile, "4.0", "AAC", 19000)
}

fn at<'a>(cmd: &'a [String], flag: &str) -> &'a str {
    &cmd[cmd.iter().position(|a| a == flag).unwrap() + 1]
}

#[test]
fn the_feeder_gets_the_fd_and_feeds_ffmpeg() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let held = std::fs::File::open("/dev/null").unwrap(); // O_CLOEXEC, like the portal's fd
    let fd = held.as_raw_fd();
    let out = dir.path().join("out");
    let sh = |c: String| vec!["sh".to_string(), "-c".into(), c];
    let gst = sh(format!("test -e /proc/self/fd/{fd} && echo fd-ok || echo fd-missing"));
    let ff = sh(format!("cat > {}", out.display()));
    let mut p = start(&ff, ":0", &crate::Env::new(), Some((&gst, fd))).unwrap();
    assert_eq!(p.procs.len(), 2);
    assert!(p.ffmpeg().wait().unwrap().success());
    p.terminate(Duration::from_secs(1));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "fd-ok\n");
    let mut solo = start(&sh("exit 3".into()), ":0", &crate::Env::new(), None).unwrap();
    assert_eq!(solo.procs.len(), 1);
    assert_eq!(solo.ffmpeg().wait().unwrap().code(), Some(3));
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
    assert!(!cmd.iter().any(|a| a == "-itsoffset"));
    assert_eq!(at(&cmd, "-g"), "30");
    let late = Opts { audio_delay_ms: -250, ..o };
    let cmd = build_cmd(&neg(1920, 1080, 30, "high"), Some("192.168.49.2"), Some(22222), &late);
    assert_eq!(at(&cmd, "-itsoffset"), "-0.250");
    assert!(idx(&cmd, "-itsoffset") < idx(&cmd, "pulse")); // shifts the sound input only
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
