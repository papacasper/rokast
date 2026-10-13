//! Where the desktop's sound goes while mirroring (AUDIO=), through pactl.
//!
//!   tv (= 1, default)  a null sink "rokast" becomes the default output: streams follow it, the TV gets the sound (we
//!                      record rokast.monitor) and the laptop speakers go quiet
//!   app:<name>         only that app's streams move to "rokast" (and new ones it opens, via `pactl subscribe`)
//!   device:<sink>      record that output's monitor; nothing is moved
//!   both               record the default output's monitor (the laptop keeps playing too; rokast <= 0.4)
//!   off (= 0)          no sound to the TV
//!
//! Streams that follow the default output are not moved by hand (the sound server could remember the move). Only
//! streams pinned elsewhere (tv) or the chosen app's (app:) are moved, and they go back where they were, or to
//! @DEFAULT_SINK@ if that was the default. What changed is written to $XDG_RUNTIME_DIR/rokast/audio.json first, so
//! a stop, a SIGTERM or the next start (recover()) puts it back.
use crate::pyfmt::repr;
use crate::{update, Env};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SINK: &str = "rokast";
pub const DEFAULT_SINK: &str = "@DEFAULT_SINK@";

/// AUDIO= value -> (kind, argument).
pub fn parse_mode(value: &str) -> Result<(&'static str, String), String> {
    let v = value.trim();
    let low = v.to_lowercase();
    match low.as_str() {
        "1" | "tv" | "" => return Ok(("tv", String::new())),
        "0" | "off" => return Ok(("off", String::new())),
        "both" => return Ok(("both", String::new())),
        _ => {}
    }
    for kind in ["app", "device"] {
        let pre = format!("{kind}:");
        if low.starts_with(&pre) {
            let arg = v[pre.len()..].trim();
            if !arg.is_empty() {
                return Ok((kind, arg.to_string()));
            }
        }
    }
    Err(format!("AUDIO must be tv, both, off, app:<name> or device:<output> (got {}; see `rokast audio`)", repr(value)))
}

fn prop<'a>(inp: &'a Value, key: &str) -> &'a str {
    inp.get("properties").and_then(|p| p.get(key)).and_then(Value::as_str).unwrap_or("")
}

/// A sink-input's app as shown and matched: application.name, else the binary.
pub fn app_of(inp: &Value) -> String {
    let name = prop(inp, "application.name");
    (if name.is_empty() { prop(inp, "application.process.binary") } else { name }).to_string()
}

/// Does this sink-input belong to `app`? application.name or binary, case-insensitive.
pub fn matches(inp: &Value, app: &str) -> bool {
    let want = app.to_lowercase();
    ["application.name", "application.process.binary"].iter().any(|k| prop(inp, k).to_lowercase() == want)
}

fn int(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(-1)
}

/// [(input index, its sink index)] that should move onto our sink (index `sink_index`).
pub fn to_move(inputs: &[Value], kind: &str, app: &str, sink_index: i64) -> Vec<(i64, i64)> {
    inputs
        .iter()
        .filter(|i| int(i, "sink") != sink_index && (kind == "tv" || (kind == "app" && matches(i, app))))
        .map(|i| (int(i, "index"), int(i, "sink")))
        .collect()
}

fn digits(s: &str) -> Option<i64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// `pactl list short sinks|sink-inputs` -> the JSON listing's shape (no properties): pactl < 16 has no -f json.
pub fn parse_short(text: &str, what: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for ln in text.lines() {
        let f: Vec<&str> = ln.split('\t').collect();
        let Some(index) = f.first().and_then(|s| digits(s)) else {
            continue;
        };
        if f.len() < 2 {
            continue;
        }
        out.push(if what == "sinks" {
            json!({"index": index, "name": f[1], "description": f[1]})
        } else {
            json!({"index": index, "sink": digits(f[1]).unwrap_or(-1), "properties": {}})
        });
    }
    out
}

/// Where a stream we moved goes back to: its old output if that still exists and wasn't the default, else
/// @DEFAULT_SINK@ (so it follows the default again rather than being pinned to it).
pub fn restore_target(moved_from: Option<&str>, default: &str, sinks: &BTreeSet<String>) -> String {
    match moved_from {
        Some(m) if !m.is_empty() && m != default && m != SINK && sinks.contains(m) => m.to_string(),
        _ => DEFAULT_SINK.into(),
    }
}

pub fn state_file(env: &Env) -> PathBuf {
    update::lock_file(env).with_file_name("audio.json")
}

/// pactl, run with `env` (its PATH finds pactl; tests put a fake one first).
#[derive(Clone)]
pub struct Pactl {
    pub env: Env,
}

impl Pactl {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new("pactl");
        c.args(args).env_clear().envs(&self.env).stdin(Stdio::null());
        c
    }

    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = self.cmd(args).stderr(Stdio::piped()).output().map_err(|e| format!("pactl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!("pactl {} failed: {err}", args.join(" ")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `pactl -f json list <what>`, or the short listing on pactl < 16.
    pub fn list(&self, what: &str) -> Vec<Value> {
        if let Ok(text) = self.run(&["-f", "json", "list", what]) {
            let text = if text.trim().is_empty() { "[]" } else { text.as_str() };
            if let Ok(Value::Array(v)) = serde_json::from_str(text) {
                return v;
            }
        }
        parse_short(&self.run(&["list", "short", what]).unwrap_or_default(), what)
    }

    pub fn default_sink(&self) -> String {
        match self.run(&["get-default-sink"]) {
            Ok(s) => s.trim().to_string(),
            Err(_) => self // pactl < 15
                .run(&["info"])
                .unwrap_or_default()
                .lines()
                .find_map(|ln| ln.strip_prefix("Default Sink:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        }
    }

    /// [(index, name)] in the server's order.
    pub fn sinks(&self) -> Vec<(i64, String)> {
        let name = |s: &Value| s.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        self.list("sinks").iter().map(|s| (int(s, "index"), name(s))).collect()
    }

    fn ours(&self) -> Option<i64> {
        self.sinks().into_iter().find(|(_, n)| n == SINK).map(|(i, _)| i)
    }
}

/// {outputs: [{name, description}], apps: [{app, media}]} for `rokast audio` and the app's menu.
pub fn listing(p: &Pactl) -> Value {
    let mut outs = Vec::new();
    for s in p.list("sinks") {
        let name = s.get("name").and_then(Value::as_str).unwrap_or("");
        if name != SINK {
            let desc = s.get("description").and_then(Value::as_str).unwrap_or(name);
            outs.push(json!({"name": name, "description": desc}));
        }
    }
    let (mut apps, mut seen) = (Vec::new(), BTreeSet::new());
    for i in p.list("sink-inputs") {
        let a = app_of(&i);
        if !a.is_empty() && seen.insert(a.to_lowercase()) {
            apps.push(json!({"app": a, "media": prop(&i, "media.name")}));
        }
    }
    json!({"outputs": outs, "apps": apps})
}

/// What a Router changed, as saved in audio.json.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub moved: Map<String, Value>,
}

/// Put back what a Router changed: default output, streams we moved, our sink unloaded.
pub fn restore_state(p: &Pactl, state: &State) {
    let sinks = p.sinks();
    let names: BTreeSet<String> = sinks.iter().map(|(_, n)| n.clone()).collect();
    let ours = sinks.iter().find(|(_, n)| n == SINK).map(|(i, _)| *i);
    let default = state.default.clone().unwrap_or_default();
    if names.contains(&default) && default != SINK {
        let _ = p.run(&["set-default-sink", &default]);
    }
    if let Some(ours) = ours {
        for inp in p.list("sink-inputs") {
            if int(&inp, "sink") == ours {
                let idx = int(&inp, "index").to_string();
                let from = state.moved.get(&idx).and_then(Value::as_str);
                let _ = p.run(&["move-sink-input", &idx, &restore_target(from, &default, &names)]);
            }
        }
    }
    if let Some(m) = state.module.as_deref().filter(|m| !m.is_empty()) {
        let _ = p.run(&["unload-module", m]);
    }
}

/// A previous session died without restoring: do it now. Returns true if there was something to undo.
pub fn recover(env: &Env) -> bool {
    let path = state_file(env);
    let Some(state) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) else {
        return false;
    };
    restore_state(&Pactl { env: env.clone() }, &state);
    let _ = fs::remove_file(path);
    true
}

/// route() once the TV is ready to play (returns the source for ffmpeg, or None), restore() in every exit path.
/// Both are idempotent.
pub struct Router {
    pub kind: &'static str,
    pub arg: String,
    pub env: Env,
    pub pactl: Pactl,
    pub state: Arc<Mutex<Option<State>>>,
    pub source: Option<String>,
    watch: Option<Child>,
}

impl Router {
    pub fn new(value: &str, env: &Env) -> Result<Self, String> {
        let (kind, arg) = parse_mode(value)?;
        let pactl = Pactl { env: env.clone() };
        Ok(Router { kind, arg, env: env.clone(), pactl, state: Arc::new(Mutex::new(None)), source: None, watch: None })
    }

    pub fn route(&mut self) -> Result<Option<String>, String> {
        if self.source.is_some() || self.kind == "off" {
            return Ok(self.source.clone());
        }
        self.source = match self.kind {
            "both" => Some(self.pactl.default_sink()).filter(|s| !s.is_empty()).map(|s| s + ".monitor"),
            "device" => {
                if !self.pactl.sinks().iter().any(|(_, n)| *n == self.arg) {
                    return Err(format!("AUDIO=device:{}: no such output (see `rokast audio`)", self.arg));
                }
                Some(format!("{}.monitor", self.arg))
            }
            _ => {
                self.take_over()?;
                Some(format!("{SINK}.monitor"))
            }
        };
        Ok(self.source.clone())
    }

    fn take_over(&mut self) -> Result<(), String> {
        let p = self.pactl.clone();
        let mut old = p.default_sink();
        if old == SINK {
            // left over and not recovered: fall back to any real output
            old = p.sinks().into_iter().map(|(_, n)| n).find(|n| n != SINK).unwrap_or_default();
        }
        let st = State { default: Some(old), module: None, moved: Map::new() };
        save(&self.env, &st); // before changing anything: restore() must be able to undo a half-done route
        *self.state.lock().unwrap() = Some(st);
        if p.ours().is_none() {
            let m = p.run(&[
                "load-module",
                "module-null-sink",
                &format!("sink_name={SINK}"),
                "sink_properties=device.description=\"rokast (TV)\"",
            ])?;
            let mut g = self.state.lock().unwrap();
            if let Some(st) = g.as_mut() {
                st.module = Some(m.trim().to_string());
                save(&self.env, st);
            }
        }
        let idx = p.ours().ok_or("could not create the rokast sound output (pactl load-module module-null-sink)")?;
        if self.kind == "tv" {
            p.run(&["set-default-sink", SINK])?;
            for _ in 0..10 {
                // streams that follow the default move by themselves; give them a moment
                if to_move(&p.list("sink-inputs"), "tv", "", idx).is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        for _ in 0..10 {
            // PipeWire accepts a move to a sink it has only just created and then drops it: check, and again
            move_streams(&p, &self.env, &self.state, self.kind, &self.arg, idx);
            if to_move(&p.list("sink-inputs"), self.kind, &self.arg, idx).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.kind == "app" {
            let mut child = p
                .cmd(&["subscribe"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("pactl subscribe: {e}"))?;
            let out = child.stdout.take().expect("piped");
            let (env, state, arg) = (self.env.clone(), self.state.clone(), self.arg.clone());
            // app mode: move each new stream of the app as it appears (a browser opens one per video)
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if line.contains("Event 'new' on sink-input #") {
                        move_streams(&p, &env, &state, "app", &arg, idx);
                    }
                }
            });
            self.watch = Some(child);
        }
        Ok(())
    }

    pub fn restore(&mut self) {
        if let Some(mut w) = self.watch.take() {
            unsafe { libc::kill(w.id() as libc::pid_t, libc::SIGTERM) };
            let _ = w.wait();
        }
        let mut g = self.state.lock().unwrap();
        if let Some(st) = g.take() {
            restore_state(&self.pactl, &st);
            let _ = fs::remove_file(state_file(&self.env));
        }
        self.source = None;
    }
}

impl Drop for Router {
    /// An early return or a panic between route() and restore() still puts the sound back.
    fn drop(&mut self) {
        self.restore();
    }
}

/// The source to record without moving anything (selftest): off -> None, device:<sink> -> its monitor, the rest ->
/// the default output's monitor.
pub fn plain_source(value: &str, env: &Env) -> Result<Option<String>, String> {
    let (kind, arg) = parse_mode(value)?;
    Ok(match kind {
        "off" => None,
        "device" => Some(format!("{arg}.monitor")),
        _ => Some(Pactl { env: env.clone() }.default_sink()).filter(|s| !s.is_empty()).map(|s| s + ".monitor"),
    })
}

fn save(env: &Env, st: &State) {
    let path = state_file(env);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, serde_json::to_string(st).expect("state serializes"));
}

fn move_streams(p: &Pactl, env: &Env, state: &Mutex<Option<State>>, kind: &str, app: &str, idx: i64) {
    let mut g = state.lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    let sinks = p.sinks();
    for (inp, old) in to_move(&p.list("sink-inputs"), kind, app, idx) {
        if p.run(&["move-sink-input", &inp.to_string(), SINK]).is_ok() {
            let from = sinks.iter().find(|(i, _)| *i == old).map(|(_, n)| n.clone()).unwrap_or_default();
            st.moved.insert(inp.to_string(), json!(from));
        }
    }
    save(env, st);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEAKERS: &str = "alsa_output.speakers";
    const HDMI: &str = "alsa_output.hdmi";

    #[test]
    fn test_parse_mode() {
        for (v, kind, arg) in [
            ("1", "tv", ""),
            ("", "tv", ""),
            ("TV", "tv", ""),
            ("0", "off", ""),
            ("off", "off", ""),
            ("both", "both", ""),
            ("app:Firefox", "app", "Firefox"),
            ("device: alsa_output.hdmi ", "device", "alsa_output.hdmi"),
        ] {
            assert_eq!(parse_mode(v), Ok((kind, arg.to_string())), "{v}");
        }
        for bad in ["yes", "app:", "device:", "2"] {
            assert!(parse_mode(bad).unwrap_err().contains("AUDIO must be"), "{bad}");
        }
    }

    #[test]
    fn test_app_matching() {
        let inp = json!({"index": 5, "sink": 1, "properties": {"application.name": "Firefox",
                                                               "application.process.binary": "firefox-bin"}});
        assert_eq!(app_of(&inp), "Firefox");
        assert!(matches(&inp, "firefox"));
        assert!(matches(&inp, "FIREFOX-BIN"));
        assert!(!matches(&inp, "fire"));
        assert_eq!(app_of(&json!({"properties": {"application.process.binary": "mpv"}})), "mpv");
    }

    #[test]
    fn test_to_move() {
        let ins = [
            json!({"index": 1, "sink": 7, "properties": {"application.name": "mpv"}}),
            json!({"index": 2, "sink": 9, "properties": {"application.name": "Firefox"}}),
            json!({"index": 3, "sink": 7, "properties": {"application.name": "Firefox"}}),
        ];
        assert_eq!(to_move(&ins, "tv", "", 9), [(1, 7), (3, 7)]);
        assert_eq!(to_move(&ins, "app", "firefox", 9), [(3, 7)]);
        assert_eq!(to_move(&ins, "both", "", 9), []);
    }

    #[test]
    fn test_restore_target() {
        let sinks: BTreeSet<String> = [SPEAKERS, HDMI, "rokast"].iter().map(|s| s.to_string()).collect();
        assert_eq!(restore_target(Some(HDMI), SPEAKERS, &sinks), HDMI);
        assert_eq!(restore_target(Some(SPEAKERS), SPEAKERS, &sinks), DEFAULT_SINK); // don't pin it
        assert_eq!(restore_target(Some("gone"), SPEAKERS, &sinks), DEFAULT_SINK);
        assert_eq!(restore_target(None, SPEAKERS, &sinks), DEFAULT_SINK);
    }

    #[test]
    fn test_parse_short() {
        assert_eq!(
            parse_short("61\talsa_output.x\tPipeWire\ts32le 2ch 48000Hz\tIDLE\n", "sinks"),
            [json!({"index": 61, "name": "alsa_output.x", "description": "alsa_output.x"})]
        );
        assert_eq!(
            parse_short("10148\t61\t10147\tPipeWire\ts16le 2ch 48000Hz\n\n", "sink-inputs"),
            [json!({"index": 10148, "sink": 61, "properties": {}})]
        );
    }

    #[test]
    fn test_recorded_pipewire_listing() {
        // Shape of a real `pactl -f json list sink-inputs` entry (PipeWire 1.6.8, trimmed).
        let inp = json!({"index": 10130, "driver": "PipeWire", "client": "10129", "sink": 61, "corked": false,
                         "properties": {"application.name": "rokast-test-a", "application.process.binary": "pacat",
                                        "media.name": "tone.wav", "node.name": "rokast-test-a"}});
        assert_eq!(to_move(std::slice::from_ref(&inp), "app", "rokast-test-a", 99), [(10130, 61)]);
        assert_eq!(app_of(&inp), "rokast-test-a");
    }
}
