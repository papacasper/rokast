//! Runs tests/parity/cases.json through rokast-core and compares each answer with tests/parity/expected.jsonl.
use rokast_core::capture::{self, Monitor, Opts};
use rokast_core::wfd::{self, Negotiated};
use rokast_core::{audio, conf, p2p, selftest, update};
use serde_json::{json, Value};
use std::collections::BTreeSet;

type Res = Result<Value, String>;

fn s<'a>(a: &'a Value, k: &str) -> &'a str {
    a[k].as_str().unwrap_or_else(|| panic!("{k}: not a string"))
}

fn u(a: &Value, k: &str) -> u32 {
    a[k].as_u64().unwrap_or_else(|| panic!("{k}: not a number")) as u32
}

fn neg(n: &Value) -> Negotiated {
    Negotiated::new(
        u(n, "width"),
        u(n, "height"),
        u(n, "fps"),
        s(n, "profile"),
        s(n, "level"),
        s(n, "audio"),
        u(n, "rtp_port") as u16,
    )
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn build_cmd(a: &Value) -> Vec<String> {
    let o = &a["opts"];
    let geometry: Option<Monitor> = serde_json::from_value(o["geometry"].clone()).expect("geometry");
    let opts = Opts {
        display: s(o, "display").into(),
        encoder: s(o, "encoder").into(),
        bitrate: s(o, "bitrate").into(),
        audio: o["audio"].as_bool().expect("audio"),
        monitor: opt_str(&o["monitor"]),
        source: s(o, "source").into(),
        out: opt_str(&o["out"]),
        seconds: o["seconds"].as_u64().map(|x| x as u32),
        geometry,
    };
    let port = a["server_port"].as_u64().map(|p| p as u16);
    capture::build_cmd(&neg(&a["n"]), a["sink_ip"].as_str(), port, &opts)
}

fn negotiated(res: &str) -> Res {
    let mut v = json!(selftest::negotiated(res)?);
    v.as_object_mut().expect("object").remove("video_param");
    Ok(v)
}

fn update_message(a: &Value) -> Value {
    let r = &a["res"];
    let res = update::Check {
        current: s(r, "current").into(),
        latest: opt_str(&r["latest"]),
        newer: r["newer"].as_bool().expect("newer"),
        url: s(r, "url").into(),
        checked: r["checked"].as_bool().expect("checked"),
        enabled: r["enabled"].as_bool().expect("enabled"),
    };
    json!(update::message(&res, s(a, "root")))
}

fn missing(what: &str) -> String {
    format!("{what}: invalid address")
}

fn run(f: &str, a: &Value) -> Res {
    let strs = |k: &str| -> Vec<String> {
        a[k].as_array().expect("array").iter().map(|x| x.as_str().expect("string").to_string()).collect()
    };
    Ok(match f {
        "parse_video_formats" => json!(wfd::parse_video_formats(s(a, "s")).map_err(|e| e.0)?),
        "choose_video" => {
            let caps = wfd::parse_video_formats(s(a, "caps")).map_err(|e| e.0)?;
            json!(wfd::choose_video(&caps, &strs("prefs")).map_err(|e| e.0)?)
        }
        "format_video_param" => json!(wfd::format_video_param(u(a, "bit"), u(a, "prof"), u(a, "level"))),
        "parse_params" => json!(wfd::parse_params(s(a, "body"))),
        "build_cmd" => json!(build_cmd(a)),
        "probe_args" => json!(capture::probe_args(s(a, "encoder"))),
        "parse_monitors" => json!(capture::parse_monitors(s(a, "text"))),
        "monitor_geometry" => json!(capture::monitor_geometry(s(a, "text"), s(a, "name"))?),
        "clean_name" => json!(conf::clean_name(s(a, "name"))),
        "parse_version" => json!(update::parse_version(s(a, "v"))),
        "is_newer" => json!(update::is_newer(s(a, "latest"), s(a, "current"))),
        "update_message" => update_message(a),
        "negotiated" => negotiated(s(a, "res"))?,
        "selftest_check" => {
            let n = selftest::negotiated(s(a, "res"))?;
            json!(selftest::check(&a["probe"], &n, u(a, "seconds"), a["audio"].as_bool().expect("audio")))
        }
        "selftest_summary" => json!(selftest::summary(&a["probe"])),
        "probe_cmd" => json!(selftest::probe_cmd(s(a, "path"))),
        "parse_mode" => json!(audio::parse_mode(s(a, "value"))?),
        "app_of" => json!(audio::app_of(&a["inp"])),
        "matches" => json!(audio::matches(&a["inp"], s(a, "app"))),
        "to_move" => {
            let inputs = a["inputs"].as_array().expect("inputs");
            json!(audio::to_move(inputs, s(a, "kind"), s(a, "app"), a["sink_index"].as_i64().expect("index")))
        }
        "parse_short" => json!(audio::parse_short(s(a, "text"), s(a, "what"))),
        "restore_target" => {
            let sinks: BTreeSet<String> = strs("sinks").into_iter().collect();
            json!(audio::restore_target(a["moved_from"].as_str(), s(a, "default"), &sinks))
        }
        "p2p_mac_of" => json!(p2p::p2p_mac_of(s(a, "wifi_mac")).ok_or_else(|| missing("wifi_mac"))?),
        "iface_mac" => json!(p2p::iface_mac(s(a, "dev_addr")).ok_or_else(|| missing("dev_addr"))?),
        "assoc_ies" => json!(p2p::assoc_ies(s(a, "dev_addr"), s(a, "name")).ok_or_else(|| missing("dev_addr"))?),
        "pick" => {
            let rokus: Vec<p2p::Roku> = serde_json::from_value(a["rokus"].clone()).expect("rokus");
            json!(p2p::pick(&rokus, s(a, "name"))?)
        }
        "parse_peer" => json!(p2p::parse_peer(s(a, "text"))),
        other => panic!("unknown parity fn {other}"),
    })
}

/// JSON equality with numbers compared by value (Python wrote 30 and 30.0 alike).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// parity <cases.json> <expected.jsonl>: every case must give the answer the Python version gave (expected.jsonl,
/// frozen before Python was removed). An error's text only counts when the case says "exact_err".
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [cases, expected] = &a[..] else { panic!("usage: parity <cases.json> <expected.jsonl>") };
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(cases).expect("read cases")).expect("cases json");
    let text = std::fs::read_to_string(expected).expect("read expected");
    let want: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("expected json")).collect();
    assert_eq!(cases.len(), want.len(), "parity: {} cases, {} expected answers", cases.len(), want.len());
    let mut bad = 0;
    for (i, (c, w)) in cases.iter().zip(&want).enumerate() {
        let got = match run(s(c, "fn"), &c["args"]) {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e }),
        };
        if got.get("err").is_some() && w.get("err").is_some() && c.get("exact_err") != Some(&json!(true)) {
            continue;
        }
        if !same(&got, w) {
            bad += 1;
            let args = c["args"].to_string();
            println!("#{i} {} {}\n  expected: {w}\n  rust:     {got}", s(c, "fn"), &args[..args.len().min(200)]);
        }
    }
    println!("parity: {}/{} cases match", cases.len() - bad, cases.len());
    std::process::exit(i32::from(bad > 0));
}
