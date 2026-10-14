//! `rokast doctor [--report] [--fix]`: checks everything `start` needs. --report: plus
//! system info, the last join failure and a selftest, for a hardware report issue. --fix: put back the sound routing
//! and screen mode a crashed session left behind.
use crate::Ctx;
use rokast_core::pyfmt::repr;
use rokast_core::{audio, capture, conf, extend, fit, joinlog, lan, paths, proc, selftest, update, wayland};
use std::path::Path;
use std::time::Duration;

struct Checks {
    ok: bool,
}

impl Checks {
    fn chk(&mut self, label: &str, cond: bool, extra: &str) {
        let more = if !extra.is_empty() && !cond { format!("  ({extra})") } else { String::new() };
        println!("{}{label}{more}", if cond { "ok    " } else { "FAIL  " });
        self.ok &= cond;
    }
}

pub fn run(ctx: &Ctx, args: &[String]) -> ! {
    let report = args[1..].iter().any(|a| a == "--report");
    let fix = args[1..].iter().any(|a| a == "--fix");
    let conf = ctx.load_conf();
    let mode = ctx.capture_mode(&conf);
    let env = &ctx.env;
    if report {
        println!("rokast {}", ctx.version());
        for (k, v) in selftest::system_info(env) {
            println!("{:<13}{v}", format!("{k}:"));
        }
    }
    let mut c = Checks { ok: true };
    for t in ["ffmpeg", "wpa_cli", "dhcpcd", "iw"] {
        c.chk(&format!("{t} installed"), proc::which(t, env).is_some(), "");
    }
    c.chk(&format!("capture: {mode}"), true, "");
    if mode == "wayland" {
        c.chk("gst-launch-1.0 installed", proc::which("gst-launch-1.0", env).is_some(), "install gstreamer");
        let gst = proc::run(&["gst-inspect-1.0", "pipewiresrc"], &[], Duration::from_secs(30));
        c.chk(
            "GStreamer pipewiresrc",
            gst.is_some_and(|o| o.ok()),
            "install gst-plugin-pipewire / gstreamer1.0-pipewire",
        );
        match wayland::ScreenCast::new().and_then(|s| s.prop("version")) {
            Ok(v) => c.chk(&format!("screen-sharing portal v{v}"), true, ""),
            Err(e) => c.chk("screen-sharing portal", false, &e),
        }
        if conf["SCREEN"] == "extend" {
            let types = wayland::ScreenCast::new().and_then(|s| s.prop("AvailableSourceTypes")).unwrap_or(0);
            if wayland::source_type(true, types) == 1 {
                println!("note  extend: this desktop's portal can't add a screen (GNOME and KDE can); mirroring");
            } else {
                c.chk("extend: the desktop adds a virtual screen", true, "");
            }
        }
    }
    let file = &conf["_file"];
    let note = if Path::new(file).exists() { "" } else { " (not created yet: defaults)" };
    println!("config: {file}{note}");
    println!("name on TV: {}", conf["NAME"]);
    if mode == "x11" {
        match capture::monitor_geometry(&capture::x11_monitors(&conf["DISPLAY"], env), &conf["MONITOR"]) {
            Ok(Some(g)) => c.chk(&format!("monitor {} {}x{}", g.name, g.width, g.height), true, ""),
            Ok(None) => c.chk("monitor: whole screen (xrandr found none)", true, ""),
            Err(e) => c.chk("monitor", false, &e),
        }
    }
    let sup = paths::find_supplicant(env, &ctx.root, paths::is_executable);
    let state = paths::supplicant_state(sup.as_deref(), &ctx.root);
    let at = sup.as_ref().map(|s| format!(": {}", s.display())).unwrap_or_default();
    let why = if state == "outdated" { " (the patch changed since it was built)" } else { "" };
    let usable = matches!(state, "ok" | "external" | "legacy");
    c.chk(&format!("patched wpa_supplicant {state}{at}"), usable, &format!("run scripts/build-supplicant{why}"));
    if state == "legacy" {
        println!(
            "note  patched wpa_supplicant is in the old in-tree location; run scripts/build-supplicant to move it"
        );
    }
    match capture::pick_encoder(&conf["ENCODER"]) {
        Ok(enc) => {
            c.chk(&format!("encoder {enc} (ENCODER={})", conf["ENCODER"]), true, "");
            if enc == "libopenh264" {
                println!("note  {}", capture::BETTER_ENCODER);
            }
        }
        Err(e) => c.chk("H.264 encoder", false, &e),
    }
    if let Err(e) = audio::parse_mode(&conf["AUDIO"]) {
        c.chk("AUDIO setting", false, &e);
    }
    // a running session has a state file too; only a leftover one is a problem
    if audio::state_file(env).exists() && !update::mirroring(env) {
        if fix && audio::recover(env) {
            c.chk("sound routing a crashed session left behind: put back", true, "");
        } else {
            c.chk("sound routing", false, "a crashed session left the sound on rokast: run rokast doctor --fix");
        }
    }
    if let Err(e) = capture::parse_fps(&conf["FPS"]) {
        c.chk("FPS setting", false, &e);
    }
    if let Err(e) = conf::audio_delay(&conf["AUDIO_DELAY"]) {
        c.chk("AUDIO_DELAY setting", false, &e);
    }
    if let Err(e) = fit::parse_mode(&conf["FIT"]) {
        c.chk("FIT setting", false, &e);
    }
    if fit::state_file(env).exists() && !update::mirroring(env) {
        if fix && fit::recover(env) {
            c.chk("screen mode a crashed session left behind: put back", true, "");
        } else {
            c.chk("screen mode", false, "a crashed session left the screen at the TV's size: run rokast doctor --fix");
        }
    }
    match (extend::parse_screen(&conf["SCREEN"]), extend::parse_side(&conf["EXTEND_SIDE"])) {
        (Err(e), _) | (_, Err(e)) => c.chk("SCREEN setting", false, &e),
        (Ok("extend"), _) if mode == "x11" => match extend::probe(&conf["DISPLAY"], env) {
            Ok(port) => c.chk(&format!("extend: full, on {port}"), true, ""),
            Err(e) => c.chk(&format!("extend: basic (windows are dragged over by hand): {e}"), true, ""),
        },
        _ => {}
    }
    if extend::state_file(env).exists() && !update::mirroring(env) {
        if fix && extend::recover(env) {
            c.chk("TV screen a crashed session left behind: removed", true, "");
        } else {
            c.chk("TV screen", false, "a crashed session left the TV screen in place: run rokast doctor --fix");
        }
    }
    match lan::p2p_dev_iface(env) {
        Ok(_) => c.chk("p2p-dev interface", true, ""),
        Err(e) => c.chk("p2p-dev interface", false, &e),
    }
    let rokus = match lan::lan_rokus(None, env) {
        Ok(r) => {
            c.chk("Rokus on LAN", !r.is_empty(), "none answered ECP :8060");
            r
        }
        Err(e) => {
            c.chk("Rokus on LAN", false, &e); // no Wi-Fi interface to find the LAN from
            vec![]
        }
    };
    let target = &conf["TARGET"];
    if !target.is_empty() {
        let found = rokus.iter().any(|r| r.name.to_lowercase() == target.to_lowercase());
        c.chk(&format!("target {} on LAN", repr(target)), found, "");
    }
    if report {
        let last = joinlog::latest(env);
        println!("last join failure: {}", last.as_ref().map(|p| p.display().to_string()).unwrap_or("none kept".into()));
        if let Some(dir) = last {
            println!("{}", std::fs::read_to_string(dir.join("summary.json")).unwrap_or_default());
        }
        let log = |m: &str| println!("{m}");
        match selftest::run(&conf, mode, 5, None, env, &log) {
            Ok(ok) => c.chk("selftest", ok, ""),
            Err(e) => c.chk("selftest", false, &e),
        }
    }
    std::process::exit(if c.ok { 0 } else { 1 })
}
