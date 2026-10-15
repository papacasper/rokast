//! Joining the TV's Wi-Fi Direct group: the part that talks to the two supplicants, iw, ip
//! and dhcpcd (see p2p.rs for the allowlist and playbooks/mirror-laptop-to-roku.md for why each step exists):
//!   1. NetworkManager's wpa_supplicant (p2p-dev-<wifi>) finds the TV and sends a PBC provision discovery, which
//!      opens a WPS push-button window on the TV's always-on group.
//!   2. Our patched wpa_supplicant (scripts/build-supplicant) on our own P2P-client iface joins that group by WPS,
//!      with a phone-like P2P IE + WFD IE in the assoc request; the Roku only opens RTSP to sources joining so.
//!   3. dhcpcd on the group iface; the TV then connects to our :7236 within ~0.5 s.
//!
//! `Radio`'s provided methods are the real steps; tests override them.
use crate::joinlog::{JoinLog, KEEP};
use crate::p2p::{assoc_ies, iface_mac, parse_peer, Roku, CTRL, DEVICE_TYPE, OWN, WFD_SOURCE_IE};
use crate::stop::{self, Fail};
use crate::{lan, paths, Env};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub type Peer = BTreeMap<String, String>;

const SH: Duration = Duration::from_secs(30);
const PEER_TIMEOUT: u64 = 30;
/// Extra time for a join caught mid-way (2026-10-14: Living Room's WPS was ready only on the 2nd scan, ~15 s after
/// the push, and the 4-way handshake finished 0.2 s after the old deadline, on every attempt).
const JOIN_GRACE: Duration = Duration::from_secs(10);

/// wpa_supplicant states between "found the TV's group" and COMPLETED (connected).
pub fn joining(state: &str) -> bool {
    matches!(state, "AUTHENTICATING" | "ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE" | "GROUP_HANDSHAKE")
}

/// A found peer we can join: it has a group (interface) address. The peer is picked by its P2P address, which comes
/// from the TV's own Wi-Fi MAC, so that is the identity; its Wi-Fi Direct name can differ from its LAN name
/// (2026-10-15: `43" onn. Roku TV` calls itself "onn Roku TV - SAL"), so the name is not checked.
pub fn peer_ready(info: &Peer) -> bool {
    info.get("interface_addr").is_some_and(|a| a != "00:00:00:00:00:00")
}

/// Our group iface: `p2p-<wifi>-mir`, cut to the 15-character netdev limit.
pub fn group_iface_of(wifi: &str) -> String {
    format!("p2p-{wifi}-mir").chars().take(15).collect()
}

/// The TV's power mode and foreground app over ECP, for join-failure evidence ({} if it does not answer).
pub fn tv_state(ip: &str) -> Map<String, Value> {
    let mut st = Map::new();
    if let Some(t) = lan::ecp_get(ip, "device-info") {
        let v = lan::tag(&t, "power-mode");
        st.insert("power_mode".into(), json!(if t.contains("<power-mode>") { v } else { "?".into() }));
    }
    if let Some(t) = lan::ecp_get(ip, "active-app") {
        let app = t.find("<app").and_then(|i| {
            let rest = &t[i..];
            let body = &rest[rest.find('>')? + 1..];
            Some(body[..body.find("</app>")?].to_string())
        });
        st.insert("app".into(), json!(app.unwrap_or_else(|| "?".into())));
    }
    st
}

/// `sudo tee path` with `data` on stdin (our supplicant's config in its root-owned folder, debugfs files).
pub fn sudo_write(path: &str, data: impl AsRef<[u8]>, env: &Env) -> Result<(), String> {
    let askpass = env.get("SUDO_ASKPASS").is_some_and(|v| !v.is_empty());
    let mut c = Command::new("sudo");
    if askpass {
        c.arg("-A");
    }
    let mut child = c
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("sudo tee {path}: {e}"))?;
    child.stdin.take().expect("piped").write_all(data.as_ref()).map_err(|e| format!("sudo tee {path}: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("sudo tee {path}: {e}"))?;
    if !out.status.success() {
        return Err(format!("sudo tee {path}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

pub trait Radio {
    fn env(&self) -> &Env;
    fn root(&self) -> PathBuf;

    fn sh(&mut self, argv: &[&str], check: bool, timeout: Duration) -> Result<String, String> {
        lan::sh_t(argv, check, timeout, self.env())
    }

    /// wpa_cli on NetworkManager's supplicant.
    fn wpa(&mut self, dev: &str, args: &[&str], check: bool) -> Result<String, String> {
        let mut a = vec!["sudo", "wpa_cli", "-p", CTRL, "-i", dev];
        a.extend(args);
        self.sh(&a, check, SH)
    }

    /// wpa_cli on our patched supplicant.
    fn own(&mut self, iface: &str, args: &[&str], check: bool) -> Result<String, String> {
        let ctrl = format!("{OWN}/ctrl");
        let mut a = vec!["sudo", "wpa_cli", "-p", &ctrl, "-i", iface];
        a.extend(args);
        self.sh(&a, check, SH)
    }

    fn own_log(&mut self) -> String {
        self.sh(&["sudo", "cat", &format!("{OWN}/supplicant.log")], false, SH).unwrap_or_default()
    }

    fn group_iface(&mut self) -> Result<String, String> {
        Ok(group_iface_of(&lan::wifi_iface(self.env())?))
    }

    fn tv_state(&mut self, ip: &str) -> Map<String, Value> {
        tv_state(ip)
    }

    /// Advertise as a WFD source on NM's p2p-dev (carried in our provision discovery request).
    fn set_wfd(&mut self, dev: &str, name: &str) -> Result<(), String> {
        self.wpa(dev, &["set", "device_name", name], true)?;
        self.wpa(dev, &["set", "wifi_display", "1"], true)?;
        self.wpa(dev, &["wfd_subelem_set", "0", WFD_SOURCE_IE], true)?;
        self.wpa(dev, &["set", "device_type", DEVICE_TYPE], true)?;
        Ok(())
    }

    /// Wait until the allowlisted target is visible as a P2P peer with a group address; its peer info.
    fn find_peer(&mut self, dev: &str, target: &Roku) -> Result<Peer, Fail> {
        self.wpa(dev, &["p2p_find"], true)?;
        let t0 = Instant::now();
        let mut look = || -> Result<Peer, Fail> {
            while t0.elapsed() < Duration::from_secs(PEER_TIMEOUT) {
                let peers = self.wpa(dev, &["p2p_peers"], false)?.to_lowercase(); // empty list -> rc 255
                if peers.split_whitespace().any(|p| p == target.p2p_mac) {
                    let info = parse_peer(&self.wpa(dev, &["p2p_peer", &target.p2p_mac], true)?);
                    if peer_ready(&info) {
                        return Ok(info);
                    }
                }
                stop::sleep(Duration::from_secs(1))?;
            }
            Err(format!("{} not visible over Wi-Fi Direct in {PEER_TIMEOUT}s", target.name).into())
        };
        let res = look();
        let _ = self.wpa(dev, &["p2p_stop_find"], false);
        res
    }

    /// Our P2P-client iface and the patched supplicant on it.
    fn start_own(&mut self, iface: &str, mac: &str) -> Result<(), Fail> {
        let sup = paths::find_supplicant(self.env(), &self.root(), paths::is_executable)
            .ok_or("patched wpa_supplicant missing: run scripts/build-supplicant")?;
        let wifi = lan::wifi_iface(self.env())?;
        self.sh(&["sudo", "iw", "dev", &wifi, "interface", "add", iface, "type", "__p2pcl"], true, SH)?;
        self.sh(&["sudo", "iw", "dev", iface, "set", "type", "__p2pcl"], true, SH)?; // `interface add` alone: managed
        self.sh(&["sudo", "ip", "link", "set", iface, "address", mac], true, SH)?;
        self.sh(&["nmcli", "device", "set", iface, "managed", "no"], false, SH)?;
        self.sh(&["sudo", "ip", "link", "set", iface, "up"], true, SH)?;
        self.sh(&["sudo", "rm", "-rf", OWN], true, SH)?;
        self.sh(&["sudo", "mkdir", "-p", "-m", "0700", OWN], true, SH)?;
        let conf = format!("{OWN}/wpa.conf");
        sudo_write(&conf, format!("ctrl_interface={OWN}/ctrl\nupdate_config=0\np2p_disabled=1\n"), self.env())?;
        let (log, pid, sup) =
            (format!("{OWN}/supplicant.log"), format!("{OWN}/pid"), sup.to_string_lossy().into_owned());
        // WPA_FORCE_P2P_GROUP: patched behaviour, keep the iface in P2P-client mode at association
        let argv = ["sudo", "env", "WPA_FORCE_P2P_GROUP=1", &sup, "-B", "-K", "-d", "-t", "-D", "nl80211", "-i", iface];
        let mut argv = argv.to_vec();
        argv.extend(["-c", &conf, "-f", &log, "-P", &pid]);
        self.sh(&argv, true, SH)?;
        let t0 = Instant::now();
        while !self.own(iface, &["status"], false)?.contains("wpa_state") {
            if t0.elapsed() > Duration::from_secs(5) {
                return Err(format!("patched wpa_supplicant did not start (log: sudo cat {log})").into());
            }
            stop::sleep(Duration::from_millis(200))?;
        }
        Ok(())
    }

    fn wait_state(&mut self, iface: &str, want: &str, timeout: Duration) -> Result<bool, Fail> {
        let t0 = Instant::now();
        loop {
            let st = parse_peer(&self.own(iface, &["status"], false)?).get("wpa_state").cloned().unwrap_or_default();
            if st == want {
                return Ok(true);
            }
            // past the deadline: a join already associating gets JOIN_GRACE more, not torn down mid-handshake
            let limit = if joining(&st) { timeout + JOIN_GRACE } else { timeout };
            if t0.elapsed() >= limit {
                return Ok(false);
            }
            stop::sleep(Duration::from_millis(500))?;
        }
    }

    /// Stop our supplicant and remove every P2P group netdev. The chip allows ONE P2P-client/GO iface.
    fn disconnect(&mut self, dev: &str, iface: Option<&str>) {
        if let Some(iface) = iface.map(String::from).or_else(|| self.group_iface().ok()) {
            let _ = self.own(&iface, &["terminate"], false);
            let pid = self.sh(&["sudo", "cat", &format!("{OWN}/pid")], false, SH).unwrap_or_default();
            if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) {
                let _ = self.sh(&["sudo", "kill", &pid], false, SH);
            }
            let _ = self.sh(&["sudo", "dhcpcd", "-x", &iface], false, Duration::from_secs(10));
        }
        let prefix = format!("Interface p2p-{}-", dev.trim_start_matches("p2p-dev-"));
        let listing = self.sh(&["iw", "dev"], false, SH).unwrap_or_default();
        for line in listing.lines().map(str::trim).filter(|l| l.starts_with(&prefix)) {
            let name = line.split_whitespace().nth(1).unwrap_or_default().to_string();
            let _ = self.wpa(dev, &["p2p_group_remove", &name], false);
            let _ = self.sh(&["sudo", "dhcpcd", "-x", &name], false, Duration::from_secs(10));
            let _ = self.sh(&["sudo", "iw", "dev", &name, "del"], false, SH);
        }
    }
}

/// The real radio: wpa_cli, iw, ip, dhcpcd through sudo.
pub struct Wpa {
    pub env: Env,
    pub root: PathBuf,
}

impl Radio for Wpa {
    fn env(&self) -> &Env {
        &self.env
    }

    fn root(&self) -> PathBuf {
        self.root.clone()
    }
}

/// Make the WPS scan probe for the TV's group by name. Its beacons carry an empty SSID, which wpa_supplicant skips
/// ("SSID not known"); only a probe response names it, and a wildcard probe is not always answered (the cause of the
/// first-join-after-idle failures: link_tests.rs, playbooks/mirror-laptop-to-roku.md).
fn probe_for(r: &mut dyn Radio, iface: &str, ssid: &str) -> Result<(), String> {
    let nets = r.own(iface, &["list_networks"], true)?; // the network wps_pbc just added is the last
    let nid = nets.lines().last().and_then(|l| l.split('\t').next()).unwrap_or("0").to_string();
    r.own(iface, &["set_network", &nid, "ssid", &crate::paths::hex(ssid.as_bytes())], true)?;
    r.own(iface, &["set_network", &nid, "scan_ssid", "1"], true)?;
    Ok(())
}

/// What a failed attempt left behind, read before the next attempt wipes it (see joinlog.rs).
fn evidence(
    r: &mut dyn Radio,
    dev: &str,
    iface: &str,
    target: &Roku,
    a: &mut Map<String, Value>,
    since: f64,
) -> String {
    a.insert("own_status".into(), json!(parse_peer(&r.own(iface, &["status"], false).unwrap_or_default())));
    a.insert("dev_status".into(), json!(parse_peer(&r.wpa(dev, &["status"], false).unwrap_or_default())));
    let after = r.wpa(dev, &["p2p_peer", &target.p2p_mac], false).unwrap_or_default();
    a.insert("peer_after".into(), json!(parse_peer(&after)));
    fresh(r.own_log(), since)
}

fn epoch() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The supplicant log if this attempt wrote it (its lines start with an epoch time). An attempt that failed before
/// our supplicant started (e.g. the TV was not visible) would otherwise keep an older session's log.
pub fn fresh(log: String, since: f64) -> String {
    let first = log.split(':').next().and_then(|t| t.trim().parse::<f64>().ok());
    match first {
        Some(t) if t < since - 1.0 => {
            "(no supplicant log from this attempt: it failed before our supplicant started)\n".into()
        }
        _ => log,
    }
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    r: &mut dyn Radio,
    dev: &str,
    iface: &str,
    target: &Roku,
    name: &str,
    timeout: Duration,
    log: &JoinLog,
    a: &mut Map<String, Value>,
) -> Result<bool, Fail> {
    r.disconnect(dev, Some(iface));
    r.set_wfd(dev, name)?;
    let peer = r.find_peer(dev, target)?;
    a.insert("peer".into(), json!(peer));
    a.insert("peer_found".into(), json!(log.t()));
    let status = parse_peer(&r.wpa(dev, &["status"], true)?);
    let dev_addr = status.get("p2p_device_address").ok_or("wpa_cli status: no p2p_device_address")?;
    let mac = iface_mac(dev_addr).ok_or_else(|| format!("bad p2p_device_address {dev_addr}"))?;
    r.start_own(iface, &mac)?;
    let ies = assoc_ies(dev_addr, name).ok_or_else(|| format!("bad p2p_device_address {dev_addr}"))?;
    r.own(iface, &["vendor_elem_add", "13", &ies], true)?; // 13 = P2P assoc request
    let _ = r.wpa(dev, &["p2p_prov_disc", &target.p2p_mac, "pbc", "join"], false); // opens the TV's PBC window
    let bssid = peer.get("interface_addr").cloned().unwrap_or_default();
    r.own(iface, &["wps_pbc", &bssid], true)?;
    if let Some(ssid) = peer.get("oper_ssid").filter(|s| !s.is_empty()) {
        probe_for(r, iface, ssid)?;
    }
    a.insert("pbc".into(), json!(log.t()));
    r.wait_state(iface, "COMPLETED", timeout)
}

fn details(label: &str, at: Option<PathBuf>) -> String {
    at.map(|d| format!(" ({label}: {})", d.display())).unwrap_or_default()
}

/// Join the TV's Wi-Fi Direct group with the patched supplicant; the group iface (no IP yet).
///
/// Says "p2p: joined on attempt N/T" (tests/live_join_soak.sh reads it). Any failed attempt is kept as evidence in
/// joinlog::failures_dir(); the error or the join line says where.
#[allow(clippy::too_many_arguments)]
pub fn connect(
    r: &mut dyn Radio,
    dev: &str,
    target: &Roku,
    name: &str,
    tries: u32,
    timeout: Duration,
    say: &dyn Fn(&str),
) -> Result<String, Fail> {
    let iface = r.group_iface()?;
    let env = r.env().clone();
    let tmap = serde_json::to_value(target).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut log = JoinLog::new(&tmap);
    log.tv.insert("before".into(), json!(r.tv_state(&target.ip)));
    for n in 1..=tries {
        let mut a = Map::new();
        let since = epoch();
        a.insert("start".into(), json!(log.t()));
        match attempt(r, dev, &iface, target, name, timeout, &log, &mut a) {
            Ok(true) => {
                if n > 1 {
                    let l = r.own_log();
                    log.joined(&a, &l);
                }
                let at = log.save(&format!("joined on attempt {n}"), &env, None, KEEP);
                say(&format!("p2p: joined on attempt {n}/{tries}{}", details("failed attempts", at)));
                return Ok(iface);
            }
            Ok(false) => {}
            Err(Fail::Stop) => return Err(Fail::Stop),
            Err(Fail::Err(e)) => {
                a.insert("error".into(), json!(e));
                let l = evidence(r, dev, &iface, target, &mut a, since);
                log.failed(&a, &l);
                r.disconnect(dev, Some(&iface));
                log.tv.insert("after".into(), json!(r.tv_state(&target.ip)));
                let at = log.save(&format!("error on attempt {n}: {e}"), &env, None, KEEP);
                return Err(format!("{e}{}", details("details", at)).into());
            }
        }
        let l = evidence(r, dev, &iface, target, &mut a, since);
        log.failed(&a, &l);
        say(&format!("p2p: join attempt {n}/{tries} failed, retrying"));
    }
    r.disconnect(dev, Some(&iface));
    log.tv.insert("after".into(), json!(r.tv_state(&target.ip)));
    let at = log.save("failed", &env, None, KEEP);
    let why = "could not join the TV's Wi-Fi Direct group (Settings>System>Screen mirroring = Prompt/Always allow?)";
    Err(format!("{why}{}", details("details", at)).into())
}

/// Get a lease from the TV (GO). No default route, no DNS changes. Our IP.
pub fn dhcp(iface: &str, timeout: u64, env: &Env) -> Result<String, String> {
    lan::sh(&["sudo", "ip", "addr", "flush", "dev", iface], true, env)?;
    let t = timeout.to_string();
    let argv = ["sudo", "dhcpcd", "-4", "-1", "-t", &t, "--nogateway", "--noipv4ll", "--nohook", "resolv.conf", iface];
    lan::sh_t(&argv, true, Duration::from_secs(timeout + 10), env)?;
    let out = lan::sh(&["ip", "-4", "-o", "addr", "show", "dev", iface], true, env)?;
    let addr = out.split_once("inet ").and_then(|(_, r)| r.split_once('/')).map(|(a, _)| a.to_string());
    addr.filter(|a| a.parse::<std::net::Ipv4Addr>().is_ok()).ok_or_else(|| format!("no DHCP lease on {iface}"))
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
