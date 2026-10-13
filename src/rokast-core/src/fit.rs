//! FIT: how the laptop picture fits the TV. `bars` (default): capture scales it in and pads (black bars when the
//! shapes differ, e.g. a 16:10 laptop on a 16:9 TV). `match`: once the TV plays, switch the captured X11 output to
//! an xrandr mode of the negotiated size so the TV is filled exactly, and put the old mode back afterwards. What was
//! changed is written to $XDG_RUNTIME_DIR/rokast/fit.json first, so a crashed session is undone by the next start or
//! `rokast doctor --fix`.
use crate::capture::{self, Monitor};
use crate::pyfmt::repr;
use crate::{proc, update, Env};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// "bars" or "match" (empty = bars).
pub fn parse_mode(value: &str) -> Result<&'static str, String> {
    match value.trim() {
        "" | "bars" => Ok("bars"),
        "match" => Ok("match"),
        other => Err(format!("FIT must be bars or match (got {})", repr(other))),
    }
}

/// One output of `xrandr --query`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    pub name: String,
    pub connected: bool,
    /// (mode, rate) marked `*`
    pub current: Option<(String, String)>,
    /// each mode with its rates, in xrandr's order
    pub modes: Vec<(String, Vec<String>)>,
}

/// `xrandr --query` -> outputs. Mode lines: `   1920x1200    165.00*+  60.00 +  59.88`; a lone `+` (preferred)
/// belongs to the rate before it.
pub fn parse_query(text: &str) -> Vec<Output> {
    let mut outs: Vec<Output> = Vec::new();
    for ln in text.lines() {
        if !ln.starts_with(char::is_whitespace) {
            // "Screen 0: ..." is neither
            let mut w = ln.split_whitespace();
            if let (Some(name), Some(state @ ("connected" | "disconnected"))) = (w.next(), w.next()) {
                outs.push(Output { name: name.into(), connected: state == "connected", ..Default::default() });
            }
            continue;
        }
        let Some(out) = outs.last_mut() else { continue };
        let mut w = ln.split_whitespace();
        let Some(mode) = w.next().filter(|m| m.contains('x') && m.starts_with(|c: char| c.is_ascii_digit())) else {
            continue; // property lines of `--verbose`, never in plain --query
        };
        let mut rates: Vec<String> = Vec::new();
        for tok in w {
            let rate: String = tok.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
            if !rate.is_empty() {
                rates.push(rate.clone());
            }
            if tok.contains('*') {
                if let Some(r) = rates.last() {
                    out.current = Some((mode.to_string(), r.clone()));
                }
            }
        }
        out.modes.push((mode.to_string(), rates));
    }
    outs
}

/// The rate to use for `mode` on `out`: the first listed (xrandr's preferred order), None if it has no such mode.
pub fn pick_rate(out: &Output, mode: &str) -> Option<String> {
    out.modes.iter().find(|(m, _)| m == mode).and_then(|(_, r)| r.first().cloned())
}

/// The rate of `mode` on `out` nearest `hz`, if one is within 1 Hz (60 Hz for FPS=60: 165/60 spaces frames unevenly).
pub fn rate_near(out: &Output, mode: &str, hz: f64) -> Option<String> {
    let (_, rates) = out.modes.iter().find(|(m, _)| m == mode)?;
    let dist = |r: &String| r.parse::<f64>().map_or(f64::MAX, |v| (v - hz).abs());
    rates.iter().min_by(|a, b| dist(a).total_cmp(&dist(b))).filter(|r| dist(r) <= 1.0).cloned()
}

/// What a Fitter changed, as saved in fit.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub display: String,
    pub output: String,
    pub mode: String,
    pub rate: String,
}

pub fn state_file(env: &Env) -> PathBuf {
    update::lock_file(env).with_file_name("fit.json")
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

fn set_mode(st: &State, env: &Env) -> Result<String, String> {
    xrandr(&st.display, env, &["--output", &st.output, "--mode", &st.mode, "--rate", &st.rate])
}

/// Put back the mode a Fitter switched away from.
pub fn restore_state(st: &State, env: &Env) -> Result<(), String> {
    set_mode(st, env).map(|_| ())
}

/// A previous session died without restoring: do it now. Returns true if there was something to undo.
pub fn recover(env: &Env) -> bool {
    let path = state_file(env);
    let Some(st) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) else {
        return false;
    };
    let _ = restore_state(&st, env);
    let _ = fs::remove_file(path);
    true
}

/// apply() once the TV plays, restore() in every exit path (and Drop). Both are idempotent.
pub struct Fitter {
    pub mode: &'static str,
    display: String,
    env: Env,
    saved: Option<State>,
}

impl Fitter {
    pub fn new(value: &str, display: &str, env: &Env) -> Result<Self, String> {
        Ok(Fitter { mode: parse_mode(value)?, display: display.into(), env: env.clone(), saved: None })
    }

    /// FIT=match: switch `mon`'s output to `w`x`h`; `hz60` (FPS=60): at its ~60 Hz rate, which alone (FIT=bars, or
    /// already that size) switches the current mode's rate. Ok(Some((geometry to capture, rate))) when switched,
    /// Ok(None) when there is nothing to do, Err(why) when it can't (the caller keeps things as they are).
    pub fn apply(&mut self, mon: &Monitor, w: u32, h: u32, hz60: bool) -> Result<Option<(Monitor, String)>, String> {
        let resize = self.mode == "match" && (mon.width, mon.height) != (w, h);
        if !resize && !hz60 {
            return Ok(None);
        }
        let outs = parse_query(&xrandr(&self.display, &self.env, &["--query"])?);
        let out = outs
            .iter()
            .find(|o| o.name == mon.name)
            .ok_or_else(|| format!("{} is not an output (a --setmonitor area?)", mon.name))?;
        let (mode, rate) = out.current.clone().ok_or_else(|| format!("{} has no current mode", out.name))?;
        let want = if resize { format!("{w}x{h}") } else { mode.clone() };
        let (w, h) = if resize { (w, h) } else { (mon.width, mon.height) };
        let new_rate = if hz60 { rate_near(out, &want, 60.0) } else { pick_rate(out, &want) }
            .ok_or_else(|| format!("{} has no {want} mode{}", out.name, if hz60 { " at 60 Hz" } else { "" }))?;
        if (want.as_str(), new_rate.as_str()) == (mode.as_str(), rate.as_str()) {
            return Ok(None);
        }
        let st = State { display: self.display.clone(), output: out.name.clone(), mode, rate };
        let path = state_file(&self.env);
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(&path, serde_json::to_string(&st).expect("state serializes")).map_err(|e| format!("{e}"))?;
        self.saved = Some(st.clone());
        let to = State { mode: want, rate: new_rate.clone(), ..st };
        if let Err(e) = set_mode(&to, &self.env) {
            self.restore();
            return Err(e);
        }
        // X applies the mode at once, but give the monitor list a moment to catch up
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let g = capture::monitor_geometry(&capture::x11_monitors(&self.display, &self.env), &mon.name)?;
            if let Some(g) = g.filter(|g| (g.width, g.height) == (w, h)) {
                return Ok(Some((g, new_rate)));
            }
            if Instant::now() >= deadline {
                self.restore();
                return Err(format!("{} did not change to {w}x{h}", mon.name));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn restore(&mut self) {
        if let Some(st) = self.saved.take() {
            let _ = restore_state(&st, &self.env);
            let _ = fs::remove_file(state_file(&self.env));
        }
    }

    pub fn switched(&self) -> bool {
        self.saved.is_some()
    }
}

impl Drop for Fitter {
    /// An early return or a panic between apply() and restore() still puts the screen back.
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // This laptop's `xrandr --query` (Intel + NVIDIA hybrid, XFCE), trimmed.
    const QUERY: &str = "Screen 0: minimum 320 x 200, current 1920 x 1200, maximum 16384 x 16384
eDP-1 connected primary 1920x1200+0+0 (normal left inverted right x axis y axis) 344mm x 215mm
   1920x1200    165.00*+  60.00 +  59.88    59.95
   1920x1080     60.01    59.97    59.96    59.93
   1600x1200     85.00    75.00    70.00    65.00    60.00
   1280x720      60.00    59.99    59.86    59.74
DP-1 disconnected (normal left inverted right x axis y axis)
HDMI-2 disconnected (normal left inverted right x axis y axis)
HDMI-1-0 disconnected (normal left inverted right x axis y axis)
";

    #[test]
    fn parses_this_laptops_listing() {
        let outs = parse_query(QUERY);
        assert_eq!(outs.len(), 4);
        let e = &outs[0];
        assert_eq!((e.name.as_str(), e.connected), ("eDP-1", true));
        assert_eq!(e.current, Some(("1920x1200".into(), "165.00".into())));
        assert_eq!(e.modes[0].1, ["165.00", "60.00", "59.88", "59.95"]); // the lone "+" is not a rate
        assert_eq!(pick_rate(e, "1920x1080").as_deref(), Some("60.01"));
        assert_eq!(pick_rate(e, "1280x720").as_deref(), Some("60.00"));
        assert_eq!(pick_rate(e, "3840x2160"), None);
        assert_eq!(rate_near(e, "1920x1200", 60.0).as_deref(), Some("60.00"));
        assert_eq!(rate_near(e, "1920x1080", 60.0).as_deref(), Some("60.01"));
        assert_eq!(rate_near(e, "1600x1200", 60.0).as_deref(), Some("60.00"));
        assert_eq!(rate_near(e, "1600x1200", 120.0), None);
        assert!(!outs[1].connected && outs[1].modes.is_empty() && outs[1].current.is_none());
    }

    #[test]
    fn current_mode_not_first() {
        let q = "HDMI-1 connected 1280x720+1920+0 (normal) 600mm x 340mm\n   1920x1080     60.00 +  50.00  \n   1280x720      60.00*   50.00  \n";
        assert_eq!(parse_query(q)[0].current, Some(("1280x720".into(), "60.00".into())));
    }

    #[test]
    fn modes() {
        assert_eq!(parse_mode(""), Ok("bars"));
        assert_eq!(parse_mode("bars"), Ok("bars"));
        assert_eq!(parse_mode(" match "), Ok("match"));
        assert_eq!(parse_mode("zoom"), Err("FIT must be bars or match (got 'zoom')".into()));
    }

    #[test]
    fn bars_and_same_size_do_nothing() {
        let env = Env::new();
        let mon = Monitor { name: "eDP-1".into(), width: 1920, height: 1080, x: 0, y: 0, primary: true };
        assert_eq!(Fitter::new("bars", ":0", &env).unwrap().apply(&mon, 1280, 720, false), Ok(None));
        assert_eq!(Fitter::new("match", ":0", &env).unwrap().apply(&mon, 1920, 1080, false), Ok(None));
    }

    #[test]
    fn state_roundtrip() {
        let st =
            State { display: ":0".into(), output: "eDP-1".into(), mode: "1920x1200".into(), rate: "165.00".into() };
        let back: State = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back, st);
        let env: Env = [("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())].into();
        assert_eq!(state_file(&env), PathBuf::from("/run/user/1000/rokast/fit.json"));
    }
}
