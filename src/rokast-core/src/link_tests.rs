//! Join-failure evidence: what `connect` keeps, where, and how many.
use super::*;
use crate::joinlog::{latest, summary_of};
use std::cell::RefCell;

/// Stands in for both supplicants: each attempt 'joins' only if its number is in `joins_on`.
struct FakeRadio {
    env: Env,
    joins_on: Vec<u32>,
    peer_error: Option<String>,
    attempt: u32,
    own_calls: RefCell<Vec<Vec<String>>>,
}

impl FakeRadio {
    fn new(cache: &std::path::Path, joins_on: &[u32], peer_error: Option<&str>) -> Self {
        let env: Env = [("XDG_CACHE_HOME".to_string(), cache.to_string_lossy().into_owned())].into();
        FakeRadio {
            env,
            joins_on: joins_on.to_vec(),
            peer_error: peer_error.map(String::from),
            attempt: 0,
            own_calls: RefCell::new(vec![]),
        }
    }
}

impl Radio for FakeRadio {
    fn env(&self) -> &Env {
        &self.env
    }
    fn root(&self) -> PathBuf {
        PathBuf::new()
    }
    fn sh(&mut self, _: &[&str], _: bool, _: Duration) -> Result<String, String> {
        Ok(format!("supplicant log {}", self.attempt))
    }
    fn wpa(&mut self, _: &str, args: &[&str], _: bool) -> Result<String, String> {
        Ok(if args[0] == "status" { "p2p_device_address=11:22:33:44:55:66\nwpa_state=DISCONNECTED" } else { "age=3" }
            .into())
    }
    fn own(&mut self, _: &str, args: &[&str], _: bool) -> Result<String, String> {
        self.own_calls.borrow_mut().push(args.iter().map(|s| s.to_string()).collect());
        Ok(match args[0] {
            "list_networks" => "network id / ssid / bssid / flags\n0\tany\taa:bb:cc:dd:ee:fe\t[CURRENT]",
            "status" => "wpa_state=SCANNING",
            _ => "OK",
        }
        .into())
    }
    fn group_iface(&mut self) -> Result<String, String> {
        Ok("p2p-wlo1-mir".into())
    }
    fn tv_state(&mut self, _: &str) -> Map<String, Value> {
        json!({"power_mode": "DisplayOff"}).as_object().cloned().unwrap()
    }
    fn set_wfd(&mut self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn find_peer(&mut self, _: &str, _: &Roku) -> Result<Peer, Fail> {
        self.attempt += 1;
        if let Some(e) = &self.peer_error {
            return Err(e.clone().into());
        }
        let p =
            [("interface_addr", "aa:bb:cc:dd:ee:fe"), ("listen_freq", "2437"), ("oper_ssid", "DIRECT-roku-XGF-A0FA2A")];
        Ok(p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }
    fn start_own(&mut self, _: &str, _: &str) -> Result<(), Fail> {
        Ok(())
    }
    fn wait_state(&mut self, _: &str, _: &str, _: Duration) -> Result<bool, Fail> {
        Ok(self.joins_on.contains(&self.attempt))
    }
    fn disconnect(&mut self, _: &str, _: Option<&str>) {}
}

fn target() -> Roku {
    Roku {
        name: "Living Room".into(),
        ip: "192.0.2.9".into(),
        model: "R".into(),
        wifi_mac: String::new(),
        p2p_mac: "aa:bb:cc:dd:ee:ff".into(),
    }
}

/// (iface, what it said, error)
fn run(r: &mut FakeRadio) -> (Option<String>, String, Option<String>) {
    let out = RefCell::new(String::new());
    let say = |m: &str| out.borrow_mut().push_str(&format!("{m}\n"));
    let res = connect(r, "p2p-dev-wlo1", &target(), "Laptop", 4, Duration::from_secs(15), &say);
    let out = out.into_inner();
    match res {
        Ok(i) => (Some(i), out, None),
        Err(Fail::Err(e)) => (None, out, Some(e)),
        Err(Fail::Stop) => panic!("not stopped"),
    }
}

#[test]
fn test_all_attempts_fail_keeps_every_attempt() {
    let t = tempfile::tempdir().unwrap();
    let mut r = FakeRadio::new(t.path(), &[], None);
    let (iface, _, err) = run(&mut r);
    assert_eq!(iface, None);
    let d = latest(&r.env).unwrap();
    assert!(err.unwrap().contains(&format!("(details: {})", d.display())));
    let s = summary_of(&d).unwrap();
    assert_eq!(s["result"], "failed");
    let n: Vec<i64> = s["attempts"].as_array().unwrap().iter().map(|a| a["attempt"].as_i64().unwrap()).collect();
    assert_eq!(n, [1, 2, 3, 4]);
    assert_eq!(s["attempts"][0]["own_status"], json!({"wpa_state": "SCANNING"}));
    assert_eq!(s["attempts"][0]["peer"]["listen_freq"], "2437");
    assert_eq!(s["tv"], json!({"before": {"power_mode": "DisplayOff"}, "after": {"power_mode": "DisplayOff"}}));
    assert_eq!(s["target"]["name"], "Living Room");
    for n in 1..=4 {
        // each attempt's own log, not the last one four times
        assert_eq!(std::fs::read_to_string(d.join(format!("attempt-{n}.log"))).unwrap(), format!("supplicant log {n}"));
    }
}

#[test]
fn test_join_after_a_retry_is_kept_too() {
    let t = tempfile::tempdir().unwrap();
    let mut r = FakeRadio::new(t.path(), &[2], None);
    let (iface, out, _) = run(&mut r);
    assert_eq!(iface.as_deref(), Some("p2p-wlo1-mir"));
    let d = latest(&r.env).unwrap();
    assert!(out.contains(&format!("p2p: joined on attempt 2/4 (failed attempts: {})", d.display())), "{out}");
    let s = summary_of(&d).unwrap();
    assert_eq!(s["result"], "joined on attempt 2");
    let failed: Vec<bool> = s["attempts"].as_array().unwrap().iter().map(|a| a.get("failed_at").is_some()).collect();
    assert_eq!(failed, [true, false]); // the join is kept to compare
    let mut files: Vec<String> =
        std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["attempt-1.log", "attempt-2.log", "summary.json"]);
}

#[test]
fn test_first_try_join_keeps_nothing() {
    let t = tempfile::tempdir().unwrap();
    let mut r = FakeRadio::new(t.path(), &[1], None);
    let (iface, out, _) = run(&mut r);
    assert_eq!(iface.as_deref(), Some("p2p-wlo1-mir"));
    assert!(out.contains("p2p: joined on attempt 1/4\n"));
    assert_eq!(latest(&r.env), None);
}

#[test]
fn test_tv_not_visible_is_kept_with_its_error() {
    let t = tempfile::tempdir().unwrap();
    let mut r = FakeRadio::new(t.path(), &[], Some("Living Room not visible over Wi-Fi Direct in 30s"));
    let (_, _, err) = run(&mut r);
    let d = latest(&r.env).unwrap();
    assert_eq!(err.unwrap(), format!("Living Room not visible over Wi-Fi Direct in 30s (details: {})", d.display()));
    let s = summary_of(&d).unwrap();
    assert_eq!(s["result"], "error on attempt 1: Living Room not visible over Wi-Fi Direct in 30s");
}

#[test]
fn test_probes_for_the_tv_by_name() {
    let t = tempfile::tempdir().unwrap();
    let mut r = FakeRadio::new(t.path(), &[1], None);
    run(&mut r);
    let calls = r.own_calls.into_inner();
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ssid = crate::paths::hex(b"DIRECT-roku-XGF-A0FA2A");
    let i = |a: &[&str]| calls.iter().position(|c| *c == v(a)).unwrap_or_else(|| panic!("no {a:?} in {calls:?}"));
    i(&["set_network", "0", "ssid", &ssid]);
    assert!(i(&["wps_pbc", "aa:bb:cc:dd:ee:fe"]) < i(&["set_network", "0", "scan_ssid", "1"]));
}

#[test]
fn group_iface_name_fits_a_netdev() {
    assert_eq!(group_iface_of("wlo1"), "p2p-wlo1-mir");
    assert_eq!(group_iface_of("wlp0s20f3"), "p2p-wlp0s20f3-m");
}

#[test]
fn mid_join_states_get_grace() {
    for s in ["AUTHENTICATING", "ASSOCIATING", "ASSOCIATED", "4WAY_HANDSHAKE", "GROUP_HANDSHAKE"] {
        assert!(joining(s), "{s}");
    }
    for s in ["SCANNING", "DISCONNECTED", "INACTIVE", "COMPLETED", ""] {
        assert!(!joining(s), "{s}");
    }
}

#[test]
fn peer_ready_needs_a_group_address_not_the_lan_name() {
    let tv = parse_peer("4e:50:dd:21:8b:c4\ndevice_name=onn Roku TV - SAL\ninterface_addr=46:50:dd:21:8b:c4\n");
    assert!(peer_ready(&tv)); // LAN name: 43" onn. Roku TV
    let no_group = parse_peer("4e:50:dd:21:8b:c4\ndevice_name=x\ninterface_addr=00:00:00:00:00:00\n");
    assert!(!peer_ready(&no_group));
    assert!(!peer_ready(&parse_peer("4e:50:dd:21:8b:c4\n")));
}

#[test]
fn an_older_sessions_supplicant_log_is_not_kept() {
    let old = "1791944034.094705: wpa_supplicant v2.11\n".to_string();
    assert!(fresh(old.clone(), 1792000000.0).starts_with("(no supplicant log"));
    assert_eq!(fresh(old.clone(), 1791944030.0), old);
    assert_eq!(fresh("supplicant log 1".into(), 1792000000.0), "supplicant log 1");
    // no time: kept
}
