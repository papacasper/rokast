//! Where the rokast CLI lives, how to run it, its config file, and the UI's remembered choices.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize, Default)]
pub struct Prefs {
    pub target: String,
    pub res: u32,
    pub bitrate: u32,
    pub audio: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub monitor: String,
    /// Window size and the Details pane, as the user left them (0 = default size).
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
    #[serde(default)]
    pub details: bool,
}

pub fn rokast_script() -> PathBuf {
    if let Ok(p) = std::env::var("ROKAST") {
        return p.into();
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    exe.ancestors()
        .map(|d| d.join("scripts/rokast"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| "rokast".into())
}

pub fn rokast_cmd(args: &[&str]) -> Command {
    let script = rokast_script();
    let mut c = Command::new(&script);
    c.args(args).stdin(Stdio::null());
    // No terminal to type a sudo password into: prompt graphically (no-op with NOPASSWD sudo).
    if std::env::var_os("SUDO_ASKPASS").is_none() {
        if let Some(askpass) = script.parent().map(|d| d.join("askpass")).filter(|p| p.is_file()) {
            c.env("SUDO_ASKPASS", askpass);
        }
    }
    c
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

pub fn prefs_path() -> PathBuf {
    config_home().join("rokast/ui.json")
}

pub fn load_prefs() -> Prefs {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Prefs { bitrate: 1, audio: true, ..Default::default() })
}

pub fn save_prefs(p: &Prefs) {
    let path = prefs_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(p) {
        let _ = std::fs::write(path, s);
    }
}

/// Change some remembered choices, keeping the rest.
pub fn update_prefs(f: impl FnOnce(&mut Prefs)) {
    let mut p = load_prefs();
    f(&mut p);
    save_prefs(&p);
}

/// rokast.conf as the CLI finds it (src/miracast/paths.py): $ROKAST_CONFIG, else ~/.config/rokast/rokast.conf,
/// else the pre-0.2 in-tree config/rokast.conf (the CLI migrates it on its next run).
fn conf_file() -> PathBuf {
    if let Some(p) = std::env::var_os("ROKAST_CONFIG") {
        return p.into();
    }
    let p = config_home().join("rokast/rokast.conf");
    if p.exists() {
        return p;
    }
    rokast_script().parent().map(|d| d.join("../config/rokast.conf")).unwrap_or(p)
}

/// TARGET from rokast.conf, used until the UI has remembered a choice of its own.
pub fn conf_target() -> String {
    let conf = conf_file();
    std::fs::read_to_string(conf)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next()?.trim().strip_prefix("TARGET")?.trim_start().strip_prefix('=').map(|v| v.trim().to_string()))
        .last()
        .unwrap_or_default()
}
