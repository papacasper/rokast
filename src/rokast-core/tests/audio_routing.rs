//! Router/recover/listing against a fake sound server (the rokast-fake-pactl test binary, linked in as `pactl`).
use rokast_core::audio::{self, Pactl, Router};
use rokast_core::Env;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SPEAKERS: &str = "alsa_output.speakers";
const HDMI: &str = "alsa_output.hdmi";

struct World {
    _t: tempfile::TempDir,
    state: PathBuf,
    env: Env,
    before: Value,
}

impl World {
    fn new() -> World {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let fake = Path::new(env!("CARGO_BIN_EXE_rokast-fake-pactl"));
        std::os::unix::fs::symlink(fs::canonicalize(fake).unwrap(), bin.join("pactl")).unwrap();
        let state = t.path().join("pactl.json");
        let s = json!({"default": SPEAKERS, "sinks": {"1": SPEAKERS, "2": HDMI}, "modules": {}, "next": 10,
                       "inputs": {"100": {"sink": 1, "app": "mpv", "pinned": false},
                                  "101": {"sink": 2, "app": "Firefox", "pinned": true},
                                  "102": {"sink": 1, "app": "Firefox", "pinned": false}}});
        fs::write(&state, s.to_string()).unwrap();
        let p = |x: &Path| x.to_str().unwrap().to_string();
        let env: Env = [
            ("PATH", format!("{}:{}", p(&bin), std::env::var("PATH").unwrap())),
            ("FAKE_PACTL_STATE", p(&state)),
            ("FAKE_PACTL_LOG", p(&t.path().join("log"))),
            ("XDG_RUNTIME_DIR", p(t.path())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        World { _t: t, state, env, before: s }
    }

    fn read(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.state).unwrap()).unwrap()
    }

    fn write(&self, s: &Value) {
        // whole file at once, as a sound server's state changes: the fake's watcher polls it
        let tmp = self.state.with_extension("new");
        fs::write(&tmp, s.to_string()).unwrap();
        fs::rename(tmp, &self.state).unwrap();
    }

    fn saved(&self) -> Value {
        serde_json::from_slice(&fs::read(audio::state_file(&self.env)).unwrap()).unwrap()
    }

    fn sink_of(s: &Value, inp: &str) -> String {
        let idx = s["inputs"][inp]["sink"].as_i64().unwrap().to_string();
        s["sinks"][&idx].as_str().unwrap().to_string()
    }

    fn assert_restored(&self) {
        let s = self.read();
        assert_eq!(s["default"], SPEAKERS);
        assert_eq!(s["sinks"], self.before["sinks"]);
        assert_eq!(s["modules"], json!({}));
        for (i, v) in self.before["inputs"].as_object().unwrap() {
            let got = &s["inputs"][i];
            assert_eq!((&got["sink"], &got["pinned"]), (&v["sink"], &v["pinned"]), "{i}");
        }
        assert!(!audio::state_file(&self.env).exists());
    }
}

#[test]
fn test_tv_only() {
    let w = World::new();
    let mut r = Router::new("1", &w.env).unwrap();
    assert_eq!(r.route().unwrap().as_deref(), Some("rokast.monitor"));
    assert_eq!(r.route().unwrap().as_deref(), Some("rokast.monitor")); // idempotent
    let s = w.read();
    assert_eq!(s["default"], "rokast");
    for i in s["inputs"].as_object().unwrap().keys() {
        assert_eq!(World::sink_of(&s, i), "rokast");
    }
    assert_eq!(w.saved()["moved"], json!({"101": HDMI})); // only the pinned one
    r.restore();
    r.restore();
    w.assert_restored();
}

#[test]
fn test_app_only_and_new_streams() {
    let w = World::new();
    let mut r = Router::new("app:firefox", &w.env).unwrap();
    r.route().unwrap();
    let mut s = w.read();
    assert_eq!(s["default"], SPEAKERS);
    assert_eq!(World::sink_of(&s, "100"), SPEAKERS);
    assert_eq!((World::sink_of(&s, "101"), World::sink_of(&s, "102")), ("rokast".into(), "rokast".into()));
    // let the watcher start: it is running once it logged its call and took one look at the streams
    let log = Path::new(&w.env["FAKE_PACTL_LOG"]).to_path_buf();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !fs::read_to_string(&log).unwrap_or_default().lines().any(|l| l == "subscribe") {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    s["inputs"]["103"] = json!({"sink": 1, "app": "Firefox", "pinned": false});
    s["inputs"]["104"] = json!({"sink": 1, "app": "mpv", "pinned": false});
    w.write(&s);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && World::sink_of(&w.read(), "103") != "rokast" {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(World::sink_of(&w.read(), "103"), "rokast");
    assert_eq!(World::sink_of(&w.read(), "104"), SPEAKERS);
    r.restore();
    let mut s = w.read();
    assert_eq!((&s["inputs"]["103"]["sink"], &s["inputs"]["103"]["pinned"]), (&json!(1), &json!(false)));
    let inputs = s["inputs"].as_object_mut().unwrap();
    inputs.remove("103");
    inputs.remove("104");
    w.write(&s);
    w.assert_restored();
}

#[test]
fn test_crash_then_recover() {
    let w = World::new();
    let mut r = Router::new("tv", &w.env).unwrap();
    r.route().unwrap();
    std::mem::forget(r); // a crash: neither restore() nor Drop runs
    assert_eq!(w.read()["default"], "rokast");
    assert!(audio::recover(&w.env));
    w.assert_restored();
    assert!(!audio::recover(&w.env));
}

#[test]
fn test_drop_restores() {
    let w = World::new();
    {
        let mut r = Router::new("tv", &w.env).unwrap();
        r.route().unwrap();
        assert_eq!(w.read()["default"], "rokast");
    } // an early return or a panic in start drops it
    w.assert_restored();
    assert!(!audio::recover(&w.env));
}

#[test]
fn test_plain_source() {
    let w = World::new();
    let src = |v: &str| audio::plain_source(v, &w.env).unwrap();
    assert_eq!(src("off"), None);
    assert_eq!(src(&format!("device:{HDMI}")), Some(format!("{HDMI}.monitor")));
    for v in ["1", "tv", "both", "app:mpv"] {
        assert_eq!(src(v), Some(format!("{SPEAKERS}.monitor")), "{v}");
    }
    assert!(audio::plain_source("loud", &w.env).is_err());
    w.assert_restored(); // nothing moved
}

#[test]
fn test_device_both_off() {
    let w = World::new();
    let route = |v: &str| Router::new(v, &w.env).unwrap().route();
    assert_eq!(route(&format!("device:{HDMI}")).unwrap(), Some(format!("{HDMI}.monitor")));
    assert!(route("device:nope").unwrap_err().contains("no such output"));
    assert_eq!(route("both").unwrap(), Some(format!("{SPEAKERS}.monitor")));
    assert_eq!(route("0").unwrap(), None);
    assert_eq!(w.read(), w.before);
}

#[test]
fn test_old_pactl() {
    let mut w = World::new();
    w.env.insert("FAKE_PACTL_OLD".into(), "1".into());
    let mut r = Router::new("tv", &w.env).unwrap();
    assert_eq!(r.route().unwrap().as_deref(), Some("rokast.monitor"));
    assert_eq!(w.read()["default"], "rokast");
    r.restore();
    w.assert_restored();
}

#[test]
fn test_leftover_default() {
    // A dead session left 'rokast' as the default and its state file got lost: still restore a real output.
    let w = World::new();
    let mut r = Router::new("tv", &w.env).unwrap();
    r.route().unwrap();
    fs::remove_file(audio::state_file(&w.env)).unwrap();
    *r.state.lock().unwrap() = None;
    let mut r2 = Router::new("tv", &w.env).unwrap();
    r2.route().unwrap();
    assert_eq!(w.saved()["default"], SPEAKERS);
    r2.restore();
    assert_eq!(w.read()["default"], SPEAKERS);
}

#[test]
fn test_listing() {
    let w = World::new();
    let got = audio::listing(&Pactl { env: w.env.clone() });
    let names: Vec<&str> = got["outputs"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap()).collect();
    assert_eq!(names, [SPEAKERS, HDMI]);
    let apps: Vec<&str> = got["apps"].as_array().unwrap().iter().map(|a| a["app"].as_str().unwrap()).collect();
    assert_eq!(apps, ["mpv", "Firefox"]);
}
