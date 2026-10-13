//! `rokast`: the command line (src/miracast/cli.py). Phase R3 runs everything here except `update`,
//! which goes to the Python cli.py (as does Wayland capture in start/doctor/selftest, until R4). `ROKAST_PY=1` sends every
//! command to Python, an escape hatch until R6.
mod doctor;
mod start;

use rokast_core::conf::{self, Conf};
use rokast_core::pyfmt::dumps;
use rokast_core::{capture, lan, paths, proc, selftest, update, Env};
use serde_json::json;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Duration;

const USAGE: &str = "usage: rokast {list [--json]|start [name]|name|config|doctor [--report]|selftest [--seconds N] \
                     [--out FILE]|monitors [--json]|ui [--tray]|version [--check [--json]]|update [--check] [--yes] \
                     [--rollback] [--auto]}";

/// Where rokast lives: $ROKAST_ROOT (set by scripts/rokast), else the first folder above this program with a VERSION
/// file (target/release/rokast in a clone).
fn find_root(env: &Env) -> PathBuf {
    if let Some(r) = env.get("ROKAST_ROOT").filter(|v| !v.is_empty()) {
        return r.into();
    }
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).unwrap_or_default();
    exe.ancestors().find(|d| d.join("VERSION").is_file()).map(Path::to_path_buf).unwrap_or_else(|| ".".into())
}

/// `python3 -I cli.py <args>`, replacing this process.
fn python(root: &Path, args: &[String]) -> ! {
    let cli = root.join("src/miracast/cli.py");
    let err = Command::new("python3").arg("-I").arg(&cli).args(args).exec();
    die(&format!("rokast: cannot run {}: {err}", cli.display()))
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    exit(1)
}

pub struct Ctx {
    pub root: PathBuf,
    pub env: Env,
}

impl Ctx {
    pub fn version(&self) -> String {
        std::fs::read_to_string(self.root.join("VERSION")).map(|v| v.trim().to_string()).unwrap_or_default()
    }

    /// Settings: defaults, the file, the environment; `_file` is the file's path. Moves a pre-0.3 config first.
    pub fn load_conf(&self) -> Conf {
        let (path, legacy) = paths::config_file(&self.env, &self.root);
        if let Some(legacy) = legacy {
            if let Err(e) = paths::migrate_config(&path, &legacy) {
                die(&format!("config: {e}"));
            }
        }
        let text = std::fs::read_to_string(&path).ok();
        let mut c = conf::parse(text.as_deref(), &self.env);
        if c["NAME"].is_empty() {
            c.insert("NAME".into(), default_name());
        }
        c.insert("_file".into(), path.to_string_lossy().into_owned());
        c
    }

    /// The capture mode, or hand the whole command to Python for what Rust can't do yet (Wayland, until R4).
    pub fn x11_or_python(&self, conf: &Conf, args: &[String]) -> &'static str {
        match conf::capture_mode(conf, &self.env) {
            Ok("x11") => "x11",
            _ => python(&self.root, args),
        }
    }
}

/// This computer's name: the pretty hostname (e.g. "Casper's ThinkPad"), else the hostname.
fn default_name() -> String {
    let pretty = proc::run(&["hostnamectl", "--pretty"], &[], Duration::from_secs(5)).map(|o| o.stdout);
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    [pretty.unwrap_or_default(), host]
        .iter()
        .map(|n| conf::clean_name(n))
        .find(|n| !n.is_empty())
        .unwrap_or_else(|| "Linux".into())
}

fn cmd_list(ctx: &Ctx, fmt: Option<&str>) {
    let rokus = lan::lan_rokus(None, &ctx.env).unwrap_or_else(|e| die(&format!("error: {e}")));
    if fmt == Some("--json") {
        return println!("{}", dumps(&json!(rokus)));
    }
    for r in rokus {
        println!("{:<24} {:<15} {}  p2p={}", r.name, r.ip, r.model, r.p2p_mac);
    }
}

fn cmd_monitors(ctx: &Ctx, fmt: Option<&str>) {
    let conf = ctx.load_conf();
    let mode = conf::capture_mode(&conf, &ctx.env).unwrap_or_else(|e| die(&format!("error: {e}")));
    let mons = if mode == "x11" {
        capture::parse_monitors(&capture::x11_monitors(&conf["DISPLAY"], &ctx.env))
    } else {
        vec![]
    };
    if fmt == Some("--json") {
        return println!("{}", dumps(&json!(mons)));
    }
    for m in mons {
        let primary = if m.primary { "  (primary)" } else { "" };
        println!("{:<12} {}x{}+{}+{}{primary}", m.name, m.width, m.height, m.x, m.y);
    }
}

fn cmd_selftest(ctx: &Ctx, all: &[String]) {
    let (mut seconds, mut out): (i64, Option<String>) = (5, None);
    let mut it = all[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => {
                seconds = it.next().map(String::as_str).unwrap_or("5").parse().unwrap_or_else(|_| {
                    die("usage: rokast selftest [--seconds N] [--out FILE]");
                })
            }
            "--out" => out = it.next().cloned(),
            _ => die("usage: rokast selftest [--seconds N] [--out FILE]"),
        }
    }
    if !(1..=600).contains(&seconds) {
        die("--seconds must be 1..600");
    }
    let conf = ctx.load_conf();
    let mode = ctx.x11_or_python(&conf, all);
    let log = |m: &str| println!("{m}");
    match selftest::run(&conf, mode, seconds as u32, out.as_deref(), &ctx.env, &log) {
        Ok(ok) => exit(if ok { 0 } else { 1 }),
        Err(e) => die(&format!("selftest: FAIL {e}")),
    }
}

fn cmd_version(ctx: &Ctx, args: &[String]) {
    let v = ctx.version();
    if !args.iter().any(|a| a == "--check") {
        return println!("{v}");
    }
    let enabled = ctx.load_conf()["CHECK_UPDATES"] != "0";
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let res = update::check(&v, enabled, &ctx.env, now, update::fetch);
    let root = ctx.root.to_string_lossy();
    let msg = update::message(&res, &root);
    if args.iter().any(|a| a == "--json") {
        let mut j = json!(res);
        let o = j.as_object_mut().expect("Check is an object");
        o.insert("message".into(), json!(msg));
        o.insert("packaged".into(), json!(update::packaged(&root)));
        o.insert("kind".into(), json!(update::kind(&ctx.root, &ctx.env, update::owns)));
        return println!("{}", dumps(&j));
    }
    println!("{v}");
    println!(
        "{}",
        if !msg.is_empty() {
            msg.as_str()
        } else if !res.enabled {
            "update check is off (CHECK_UPDATES=0)"
        } else if res.latest.is_some() {
            "up to date"
        } else {
            "could not reach GitHub; will try again tomorrow"
        }
    );
}

fn cmd_ui(ctx: &Ctx, args: &[String]) -> ! {
    let exe = ctx.root.join("ui/target/release/rokast-ui");
    if !paths::is_executable(&exe) {
        die("desktop UI not built: run scripts/install-ui (needs cargo and GTK 4 dev files)");
    }
    let err = Command::new(&exe).args(args).exec(); // e.g. --tray: start hidden in the system tray
    die(&format!("rokast: cannot run {}: {err}", exe.display()))
}

fn main() {
    let env = rokast_core::env();
    let root = find_root(&env);
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        args.push("help".into());
    }
    if env.get("ROKAST_PY").is_some_and(|v| !v.is_empty() && v != "0") {
        python(&root, &args);
    }
    let ctx = Ctx { root, env };
    let arg = args.get(1).map(String::as_str);
    match args[0].as_str() {
        "list" => cmd_list(&ctx, arg),
        "doctor" => doctor::run(&ctx, &args),
        "selftest" => cmd_selftest(&ctx, &args),
        "monitors" => cmd_monitors(&ctx, arg),
        "name" => println!("{}", ctx.load_conf()["NAME"]),
        "config" => println!("{}", ctx.load_conf()["_file"]),
        "version" => cmd_version(&ctx, &args[1..]),
        "ui" => cmd_ui(&ctx, &args[1..]),
        "start" => start::run(&ctx, &args),
        "update" => python(&ctx.root, &args),
        _ => die(USAGE),
    }
}
