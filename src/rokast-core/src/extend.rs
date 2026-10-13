//! SCREEN=extend: the TV becomes a second screen beside the laptop instead of a copy of it (X11).
//! "full": a free video port on an Intel/AMD GPU is made to look connected (debugfs `edid_override` + `force`, as
//! root through sudo) with an EDID of the TV's size; X then has a real output that windows open and maximise on, and
//! rokast captures its area. "basic" (no such port, debugfs blocked): the X screen grows and `--setmonitor` declares
//! the TV's area; ffmpeg captures it, but desktops ignore it for placing windows, so they are dragged over by hand.
//! What was changed is written to $XDG_RUNTIME_DIR/rokast/extend.json first, so a crash is undone by the next start
//! or `rokast doctor --fix`. XFCE's "display connected" dialog is held off while the port appears.
use crate::capture::{self, Monitor};
use crate::edid::edid;
use crate::fit::{self, Output};
use crate::pyfmt::repr;
use crate::{link, proc, update, Env};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEBUGFS: &str = "/sys/kernel/debug/dri";
const DRIVERS: [&str; 3] = ["i915", "xe", "amdgpu"]; // NVIDIA's has the files but ignores them
const BASIC: &str = "rokast-tv";

/// "mirror" or "extend" (empty = mirror).
pub fn parse_screen(value: &str) -> Result<&'static str, String> {
    match value.trim() {
        "" | "mirror" => Ok("mirror"),
        "extend" => Ok("extend"),
        other => Err(format!("SCREEN must be mirror or extend (got {})", repr(other))),
    }
}

/// Where the TV goes: right (default), left, above, below.
pub fn parse_side(value: &str) -> Result<&'static str, String> {
    match value.trim() {
        "" | "right" => Ok("right"),
        "left" => Ok("left"),
        "above" => Ok("above"),
        "below" => Ok("below"),
        other => Err(format!("EXTEND_SIDE must be right, left, above or below (got {})", repr(other))),
    }
}

/// A kernel connector (`/sys/class/drm/card1-HDMI-A-2`).
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub card: u32,
    pub name: String,
    pub connected: bool,
    pub driver: String,
}

/// The names X drivers give a kernel connector: modesetting `HDMI-2`, amdgpu `HDMI-A-2`, intel `HDMI2`.
pub fn x_names(kernel: &str) -> Vec<String> {
    let short = kernel.replacen("HDMI-A-", "HDMI-", 1);
    let mut v = vec![short.clone(), kernel.to_string(), short.replacen('-', "", 1)];
    v.dedup();
    v
}

/// Free external ports on a supported GPU that X lists as disconnected outputs -> (port, X name), HDMI first: a
/// forced DisplayPort with nothing plugged in fails link training and never shows up (this laptop's DP-1).
pub fn pick_ports<'a>(ports: &'a [Port], outs: &[Output]) -> Vec<(&'a Port, String)> {
    let mut v: Vec<_> = ports
        .iter()
        .filter(|p| !p.connected && DRIVERS.contains(&p.driver.as_str()))
        .filter_map(|p| {
            let n = x_names(&p.name).into_iter().find(|n| outs.iter().any(|o| &o.name == n && !o.connected))?;
            Some((p, n))
        })
        .collect();
    v.sort_by_key(|(p, _)| !p.name.starts_with("HDMI")); // stable: card/name order otherwise
    v
}

const NO_PORT: &str = "no free video port on an Intel or AMD GPU";

fn debugfs_dir(p: &Port, env: &Env) -> Result<String, String> {
    let dir = format!("{DEBUGFS}/{}/{}", p.card, p.name);
    crate::lan::sh(&["sudo", "test", "-e", &format!("{dir}/edid_override")], true, env)
        .map_err(|_| format!("{dir}/edid_override not reachable (debugfs not mounted, or kernel lockdown)"))?;
    Ok(dir)
}

/// External connectors from /sys/class/drm.
pub fn ports() -> Vec<Port> {
    let mut v = Vec::new();
    for e in fs::read_dir("/sys/class/drm").into_iter().flatten().flatten() {
        let f = e.file_name().to_string_lossy().to_string();
        let Some((card, name)) = f.strip_prefix("card").and_then(|r| r.split_once('-')) else { continue };
        let Ok(card) = card.parse::<u32>() else { continue };
        if !["HDMI-A-", "DP-", "DVI-"].iter().any(|t| name.starts_with(t)) {
            continue;
        }
        let status = fs::read_to_string(e.path().join("status")).unwrap_or_default();
        let driver = fs::read_link(format!("/sys/class/drm/card{card}/device/driver"))
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        v.push(Port { card, name: name.into(), connected: status.trim() == "connected", driver });
    }
    v.sort_by(|a, b| (a.card, &a.name).cmp(&(b.card, &b.name)));
    v
}

/// What an Extender changed, as saved in extend.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub display: String,
    /// the new X output (full) or the --setmonitor name (basic)
    pub output: String,
    /// the port's debugfs folder; None = basic
    pub debugfs: Option<String>,
    /// where the other outputs were, put back afterwards (left/above move them)
    pub positions: Vec<(String, i32, i32)>,
    /// the X screen size before (basic grows it)
    pub fb: (u32, u32),
    /// XFCE's displays /Notify before
    pub notify: Option<String>,
}

pub fn state_file(env: &Env) -> PathBuf {
    update::lock_file(env).with_file_name("extend.json")
}

fn xrandr(display: &str, env: &Env, args: &[&str]) -> Result<String, String> {
    let home = env.get("HOME").cloned().unwrap_or_default();
    let xauth = env.get("XAUTHORITY").filter(|v| !v.is_empty()).cloned().unwrap_or(format!("{home}/.Xauthority"));
    let mut argv = vec!["xrandr"];
    argv.extend_from_slice(args);
    match proc::run(&argv, &[("DISPLAY", display), ("XAUTHORITY", &xauth)], Duration::from_secs(15)) {
        Some(o) if o.ok() => Ok(o.stdout),
        Some(o) => Err(format!("xrandr {}: {}", args.join(" "), o.stderr.trim())),
        None => Err("xrandr not found or timed out".into()),
    }
}

fn notify(set: Option<&str>) -> Option<String> {
    let mut argv = vec!["xfconf-query", "-c", "displays", "-p", "/Notify"];
    if let Some(v) = set {
        argv.extend(["-s", v]);
    }
    proc::run(&argv, &[], Duration::from_secs(5)).filter(|o| o.ok()).map(|o| o.stdout.trim().to_string())
}

/// Undo what `st` describes. Every step runs even if one fails.
pub fn restore_state(st: &State, env: &Env) -> Result<(), String> {
    let mut errs = Vec::new();
    let mut args: Vec<String> = Vec::new();
    match &st.debugfs {
        Some(dir) => {
            args.extend(["--output".into(), st.output.clone(), "--off".into()]);
            for (n, x, y) in &st.positions {
                args.extend(["--output".into(), n.clone(), "--pos".into(), format!("{x}x{y}")]);
            }
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            errs.extend(xrandr(&st.display, env, &a).err());
            errs.extend(link::sudo_write(&format!("{dir}/force"), "unspecified", env).err());
            errs.extend(link::sudo_write(&format!("{dir}/edid_override"), "reset", env).err());
        }
        None => {
            let _ = xrandr(&st.display, env, &["--delmonitor", &st.output]);
            errs.extend(xrandr(&st.display, env, &["--fb", &format!("{}x{}", st.fb.0, st.fb.1)]).err());
        }
    }
    if let Some(v) = &st.notify {
        std::thread::sleep(Duration::from_millis(500)); // let XFCE see the port go before it may ask again
        notify(Some(v));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// A previous session died with the TV screen still there: remove it. True if there was something to undo.
pub fn recover(env: &Env) -> bool {
    let path = state_file(env);
    let Some(st) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) else {
        return false;
    };
    let _ = restore_state(&st, env);
    let _ = fs::remove_file(path);
    true
}

/// Whether "full" works here: Ok(port) or Err(why not) (then "basic").
pub fn probe(display: &str, env: &Env) -> Result<String, String> {
    let outs = fit::parse_query(&xrandr(display, env, &["--query"])?);
    let ports = ports();
    let (p, xname) = pick_ports(&ports, &outs).into_iter().next().ok_or(NO_PORT)?;
    debugfs_dir(p, env)?;
    Ok(format!("{} (X {xname})", p.name))
}

fn bounds(mons: &[Monitor]) -> (u32, u32) {
    let w = mons.iter().map(|m| m.x + m.width as i32).max().unwrap_or(0);
    let h = mons.iter().map(|m| m.y + m.height as i32).max().unwrap_or(0);
    (w.max(0) as u32, h.max(0) as u32)
}

/// apply() once the TV plays, restore() in every exit path (and Drop).
pub struct Extender {
    pub screen: &'static str,
    side: &'static str,
    display: String,
    env: Env,
    saved: Option<State>,
}

impl Extender {
    pub fn new(screen: &str, side: &str, display: &str, env: &Env) -> Result<Self, String> {
        let (screen, side) = (parse_screen(screen)?, parse_side(side)?);
        Ok(Extender { screen, side, display: display.into(), env: env.clone(), saved: None })
    }

    fn save(&mut self, st: State) -> Result<(), String> {
        let path = state_file(&self.env);
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(&path, serde_json::to_string(&st).expect("state serializes")).map_err(|e| format!("{e}"))?;
        self.saved = Some(st);
        Ok(())
    }

    /// Add a `w`x`h` screen on `side` of `anchor`. Ok(Some((its area, "full"/"basic: why"))), Ok(None) for
    /// SCREEN=mirror, Err when neither works (the caller mirrors).
    pub fn apply(&mut self, anchor: &Monitor, w: u32, h: u32) -> Result<Option<(Monitor, String)>, String> {
        if self.screen != "extend" {
            return Ok(None);
        }
        match self.full(anchor, w, h) {
            Ok(m) => Ok(Some((m, "full".into()))),
            Err(why) => {
                self.restore();
                let m = self.basic(w, h).inspect_err(|_| self.restore())?;
                Ok(Some((m, format!("basic: {why}"))))
            }
        }
    }

    /// Each free port in turn until one comes up.
    fn full(&mut self, anchor: &Monitor, w: u32, h: u32) -> Result<Monitor, String> {
        let blob = edid(w, h)?;
        let outs = fit::parse_query(&xrandr(&self.display, &self.env, &["--query"])?);
        let ports = ports();
        let cands = pick_ports(&ports, &outs);
        let mut errs = Vec::new();
        for (p, xname) in &cands {
            let dir = debugfs_dir(p, &self.env)?; // same for every port: debugfs or lockdown
            match self.try_port(anchor, w, h, &blob, &outs, dir, xname) {
                Ok(m) => return Ok(m),
                Err(e) => {
                    self.restore();
                    errs.push(e);
                }
            }
        }
        Err(if errs.is_empty() { NO_PORT.into() } else { errs.join("; ") })
    }

    #[allow(clippy::too_many_arguments)]
    fn try_port(
        &mut self,
        anchor: &Monitor,
        w: u32,
        h: u32,
        blob: &[u8],
        outs: &[Output],
        dir: String,
        xname: &str,
    ) -> Result<Monitor, String> {
        let xname = xname.to_string();
        let mons = capture::parse_monitors(&capture::x11_monitors(&self.display, &self.env));
        let positions = mons
            .iter()
            .filter(|m| outs.iter().any(|o| o.name == m.name && o.current.is_some()))
            .map(|m| (m.name.clone(), m.x, m.y))
            .collect();
        let old = notify(None);
        self.save(State {
            display: self.display.clone(),
            output: xname.clone(),
            debugfs: Some(dir.clone()),
            positions,
            fb: bounds(&mons),
            notify: old.clone(),
        })?;
        if old.is_some() {
            notify(Some("0"));
        }
        link::sudo_write(&format!("{dir}/edid_override"), blob, &self.env)?;
        link::sudo_write(&format!("{dir}/force"), "on", &self.env)?;
        let mode = format!("{w}x{h}");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // --query makes X probe the outputs again
            let outs = fit::parse_query(&xrandr(&self.display, &self.env, &["--query"])?);
            if outs.iter().any(|o| o.name == xname && o.connected && o.modes.iter().any(|(m, _)| *m == mode)) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("{xname} did not show up as connected"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let rel = format!(
            "--{}",
            if matches!(self.side, "right" | "left") { format!("{}-of", self.side) } else { self.side.into() }
        );
        xrandr(&self.display, &self.env, &["--output", &xname, "--mode", &mode, &rel, &anchor.name])?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mons = capture::parse_monitors(&capture::x11_monitors(&self.display, &self.env));
            if let Some(m) = mons.iter().find(|m| m.name == xname && (m.width, m.height) == (w, h)) {
                return Ok(m.clone());
            }
            if Instant::now() >= deadline {
                return Err(format!("{xname} did not turn on at {mode}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Grow the X screen and declare the TV's area beside it (right, or below for above/below).
    fn basic(&mut self, w: u32, h: u32) -> Result<Monitor, String> {
        let mons = capture::parse_monitors(&capture::x11_monitors(&self.display, &self.env));
        let (fw, fh) = bounds(&mons);
        let (x, y) = if matches!(self.side, "above" | "below") { (0, fh) } else { (fw, 0) };
        let (nw, nh) = ((x + w).max(fw), (y + h).max(fh));
        self.save(State {
            display: self.display.clone(),
            output: BASIC.into(),
            debugfs: None,
            positions: Vec::new(),
            fb: (fw, fh),
            notify: None,
        })?;
        let geo = format!("{w}/880x{h}/500+{x}+{y}");
        xrandr(&self.display, &self.env, &["--fb", &format!("{nw}x{nh}"), "--setmonitor", BASIC, &geo, "none"])?;
        Ok(Monitor { name: BASIC.into(), width: w, height: h, x: x as i32, y: y as i32, primary: false })
    }

    pub fn restore(&mut self) {
        if let Some(st) = self.saved.take() {
            let _ = restore_state(&st, &self.env);
            let _ = fs::remove_file(state_file(&self.env));
        }
    }
}

impl Drop for Extender {
    /// An early return or a panic between apply() and restore() still removes the TV screen.
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        assert_eq!(parse_screen(""), Ok("mirror"));
        assert_eq!(parse_screen(" extend "), Ok("extend"));
        assert_eq!(parse_screen("both"), Err("SCREEN must be mirror or extend (got 'both')".into()));
        assert_eq!(parse_side(""), Ok("right"));
        assert_eq!(parse_side("above"), Ok("above"));
        assert!(parse_side("up").is_err());
    }

    #[test]
    fn names() {
        assert_eq!(x_names("HDMI-A-2"), ["HDMI-2", "HDMI-A-2", "HDMI2"]);
        assert_eq!(x_names("DP-1"), ["DP-1", "DP1"]);
    }

    // this laptop: Intel (card1) drives X, NVIDIA (card0) is the render offload GPU
    #[test]
    fn picks_a_free_intel_port() {
        let q = "eDP-1 connected primary 1920x1200+0+0 (normal) 344mm x 215mm\n   1920x1200    165.00*+\n\
                 DP-1 disconnected (normal)\nHDMI-1 connected (normal)\n   1920x1080     60.00 +\n\
                 HDMI-2 disconnected (normal)\nHDMI-1-0 disconnected (normal)\n";
        let outs = fit::parse_query(q);
        let port = |card, name: &str, connected, driver: &str| Port {
            card,
            name: name.into(),
            connected,
            driver: driver.into(),
        };
        let ports = [
            port(0, "HDMI-A-3", false, "nvidia"),
            port(1, "DP-1", true, "i915"),
            port(1, "HDMI-A-1", false, "i915"), // kernel says free, X says connected: skip
            port(1, "HDMI-A-2", false, "i915"),
        ];
        let v = pick_ports(&ports, &outs);
        let (p, x) = &v[0];
        assert_eq!((p.card, p.name.as_str(), x.as_str()), (1, "HDMI-A-2", "HDMI-2"));
        assert_eq!(v.len(), 1);
        assert!(pick_ports(&ports[..1], &outs).is_empty(), "NVIDIA ignores the override");
    }

    #[test]
    fn state_roundtrip_and_bounds() {
        let st = State {
            display: ":0".into(),
            output: "HDMI-2".into(),
            debugfs: Some("/sys/kernel/debug/dri/1/HDMI-A-2".into()),
            positions: vec![("eDP-1".into(), 0, 0)],
            fb: (1920, 1200),
            notify: Some("1".into()),
        };
        let back: State = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back, st);
        let env: Env = [("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())].into();
        assert_eq!(state_file(&env), PathBuf::from("/run/user/1000/rokast/extend.json"));
        let m = |w, h, x, y| Monitor { name: "m".into(), width: w, height: h, x, y, primary: false };
        assert_eq!(bounds(&[m(1920, 1200, 0, 0), m(1920, 1080, 1920, 0)]), (3840, 1200));
    }
}
