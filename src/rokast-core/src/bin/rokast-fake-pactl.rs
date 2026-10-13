//! A stand-in `pactl` for tests/audio_routing.rs (test-only, not shipped): a sound server's routing in a JSON file
//! ($FAKE_PACTL_STATE).
//!
//! Behaves like PipeWire/PulseAudio >= 15: streams follow the default output unless pinned; a move to a named output
//! pins the stream, a move to @DEFAULT_SINK@ unpins it; unloading a sink sends its streams to the default.
//! FAKE_PACTL_OLD=1 acts like pactl < 15: no -f json, no get-default-sink. Every call is appended to $FAKE_PACTL_LOG.
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::time::Duration;

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn load(p: &str) -> Value {
    serde_json::from_slice(&std::fs::read(p).expect("read state")).expect("state json")
}

fn save(p: &str, s: &Value) {
    let tmp = format!("{p}.tmp");
    std::fs::write(&tmp, s.to_string()).expect("write state");
    std::fs::rename(tmp, p).expect("replace state");
}

fn obj<'a>(s: &'a mut Value, k: &str) -> &'a mut Map<String, Value> {
    s[k].as_object_mut().expect("object")
}

fn by_name(s: &Value, name: &str) -> Option<i64> {
    s["sinks"].as_object()?.iter().find(|(_, n)| n.as_str() == Some(name)).and_then(|(i, _)| i.parse().ok())
}

/// Python's str.title(): a letter after a non-letter goes upper case, the rest lower.
fn title(s: &str) -> String {
    let mut prev = false;
    s.chars()
        .map(|c| {
            let up = !prev;
            prev = c.is_alphabetic();
            if up {
                c.to_uppercase().collect::<String>()
            } else {
                c.to_lowercase().collect()
            }
        })
        .collect()
}

fn input_ids(s: &Value) -> BTreeSet<i64> {
    s["inputs"].as_object().map(|m| m.keys().filter_map(|k| k.parse().ok()).collect()).unwrap_or_default()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let p = std::env::var("FAKE_PACTL_STATE").expect("FAKE_PACTL_STATE");
    let old = std::env::var("FAKE_PACTL_OLD").as_deref() == Ok("1");
    if let Ok(log) = std::env::var("FAKE_PACTL_LOG") {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log).expect("open log");
        writeln!(f, "{}", a.join(" ")).expect("write log");
    }
    let mut s = load(&p);
    let sinks = s["sinks"].as_object().cloned().unwrap_or_default();
    let inputs = s["inputs"].as_object().cloned().unwrap_or_default();
    match a[..] {
        ["-f", "json", _, what, ..] => {
            if old {
                fail("pactl: invalid option -- 'f'");
            }
            let out: Vec<Value> = if what == "sinks" {
                sinks
                    .iter()
                    .map(|(i, n)| {
                        let n = n.as_str().unwrap_or_default();
                        json!({"index": i.parse::<i64>().unwrap(), "name": n, "description": title(n)})
                    })
                    .collect()
            } else {
                inputs
                    .iter()
                    .map(|(i, v)| {
                        let app = v["app"].as_str().unwrap_or_default();
                        json!({"index": i.parse::<i64>().unwrap(), "sink": v["sink"], "properties": {
                            "application.name": app,
                            "application.process.binary": v.get("bin").and_then(Value::as_str).unwrap_or("pacat"),
                            "media.name": format!("{app} media")}})
                    })
                    .collect()
            };
            println!("{}", Value::Array(out));
        }
        ["list", "short", what, ..] => {
            let lines: Vec<String> = if what == "sinks" {
                sinks
                    .iter()
                    .map(|(i, n)| format!("{i}\t{}\tPipeWire\ts16le 2ch 48000Hz\tIDLE", n.as_str().unwrap_or_default()))
                    .collect()
            } else {
                inputs.iter().map(|(i, v)| format!("{i}\t{}\t99\tPipeWire\ts16le 2ch 48000Hz", v["sink"])).collect()
            };
            println!("{}", lines.join("\n"));
        }
        ["get-default-sink"] => {
            if old {
                fail("No valid command specified.");
            }
            println!("{}", s["default"].as_str().unwrap_or_default());
        }
        ["info"] => {
            println!(
                "Server Name: fake\nDefault Sink: {}\nDefault Source: x",
                s["default"].as_str().unwrap_or_default()
            )
        }
        ["set-default-sink", name, ..] => {
            let Some(idx) = by_name(&s, name) else { fail("Failure: No such entity") };
            s["default"] = json!(name);
            for v in obj(&mut s, "inputs").values_mut() {
                if v["pinned"] != json!(true) {
                    v["sink"] = json!(idx);
                }
            }
            save(&p, &s);
        }
        ["move-sink-input", id, to, ..] => {
            if !inputs.contains_key(id) {
                fail("Failure: No such entity");
            }
            let target =
                if to == "@DEFAULT_SINK@" { s["default"].as_str().unwrap_or_default().to_string() } else { to.into() };
            let Some(idx) = by_name(&s, &target) else { fail("Failure: No such entity") };
            let v = &mut obj(&mut s, "inputs")[id];
            v["sink"] = json!(idx);
            v["pinned"] = json!(to != "@DEFAULT_SINK@");
            save(&p, &s);
        }
        ["load-module", ..] => {
            let name = a.iter().find_map(|x| x.strip_prefix("sink_name=")).expect("sink_name=");
            let next = s["next"].as_i64().unwrap_or(0) + 1;
            s["next"] = json!(next);
            obj(&mut s, "sinks").insert(next.to_string(), json!(name));
            obj(&mut s, "modules").insert((next + 1000).to_string(), json!(next));
            save(&p, &s);
            println!("{}", next + 1000);
        }
        ["unload-module", id, ..] => {
            let Some(sink) = obj(&mut s, "modules").shift_remove(id) else { fail("Failure: No such entity") };
            let sink = sink.as_i64().unwrap_or(-1);
            let name = obj(&mut s, "sinks").shift_remove(&sink.to_string()).unwrap_or_default();
            if s["default"] == name {
                s["default"] = s["sinks"].as_object().and_then(|m| m.values().next().cloned()).unwrap_or_default();
            }
            let def = by_name(&s, s["default"].as_str().unwrap_or_default());
            for v in obj(&mut s, "inputs").values_mut() {
                if v["sink"].as_i64() == Some(sink) {
                    v["sink"] = json!(def);
                    v["pinned"] = json!(false);
                }
            }
            save(&p, &s);
        }
        ["subscribe"] => {
            let mut seen = input_ids(&s);
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let now = input_ids(&load(&p));
                for i in now.difference(&seen) {
                    println!("Event 'new' on sink-input #{i}");
                }
                std::io::stdout().flush().ok();
                seen = now;
            }
        }
        _ => fail(&format!("fake pactl: unsupported {a:?}")),
    }
}
