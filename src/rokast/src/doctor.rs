//! `rokast doctor [--report]`: checks everything `start` needs. --report: plus
//! system info, the last join failure and a selftest, for a hardware report issue.
use crate::Ctx;
use rokast_core::pyfmt::repr;
use rokast_core::{capture, joinlog, lan, paths, proc, selftest, wayland};
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
    let report = args.get(1).is_some_and(|a| a == "--report");
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
