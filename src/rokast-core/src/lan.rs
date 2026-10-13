//! Finding Rokus on our own LAN over ECP (:8060), and the shell helpers the p2p side uses.
use crate::p2p::{p2p_mac_of, Roku, CTRL};
use crate::{proc, Env};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const ECP_TIMEOUT: Duration = Duration::from_millis(1500);

/// Run a command (at most 30 s); stdout trimmed. `sudo` gets -A when $SUDO_ASKPASS is set (the app:
/// no terminal for a password).
pub fn sh(argv: &[&str], check: bool, env: &Env) -> Result<String, String> {
    sh_t(argv, check, Duration::from_secs(30), env)
}

/// `sh` with its own time limit.
pub fn sh_t(argv: &[&str], check: bool, timeout: Duration, env: &Env) -> Result<String, String> {
    let mut argv: Vec<&str> = argv.to_vec();
    if argv.first() == Some(&"sudo") && env.get("SUDO_ASKPASS").is_some_and(|v| !v.is_empty()) {
        argv.insert(1, "-A");
    }
    let cmd = argv.join(" ");
    let out = proc::run(&argv, &[], timeout)
        .ok_or_else(|| format!("{cmd}: did not start or ran over {} s", timeout.as_secs()))?;
    let stdout = out.stdout.trim().to_string();
    if check && !out.ok() {
        let err = out.stderr.trim();
        return Err(format!(
            "{cmd} -> {}: {}",
            proc::returncode(&out.status),
            if err.is_empty() { &stdout } else { err }
        ));
    }
    Ok(stdout)
}

pub fn p2p_dev_iface(env: &Env) -> Result<String, String> {
    sh(&["sudo", "ls", CTRL], true, env)?
        .split_whitespace()
        .find(|n| n.starts_with("p2p-dev-"))
        .map(String::from)
        .ok_or_else(|| "no p2p-dev-* interface (is Wi-Fi P2P enabled in NetworkManager/wpa_supplicant?)".into())
}

pub fn wifi_iface(env: &Env) -> Result<String, String> {
    Ok(p2p_dev_iface(env)?.trim_start_matches("p2p-dev-").to_string())
}

/// `ip -4 -o addr show dev X` -> "192.168.0." (the first address's /24).
pub fn prefix_of(ip_addr_output: &str) -> Option<String> {
    let rest = &ip_addr_output[ip_addr_output.find("inet ")? + 5..];
    let addr = &rest[..rest.find('/')?];
    let parts: Vec<&str> = addr.split('.').collect();
    let ok = parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    ok.then(|| format!("{}.{}.{}.", parts[0], parts[1], parts[2]))
}

/// First `<k>...</k>` in an ECP XML answer ("" if absent).
pub fn tag(xml: &str, k: &str) -> String {
    let open = format!("<{k}>");
    let Some(i) = xml.find(&open) else { return String::new() };
    let rest = &xml[i + open.len()..];
    rest.find(&format!("</{k}>")).map(|j| rest[..j].to_string()).unwrap_or_default()
}

/// A device-info answer -> the Roku, if it reports a wifi MAC.
pub fn parse_device_info(ip: &str, xml: &str) -> Option<Roku> {
    let mac = tag(xml, "wifi-mac").to_lowercase();
    let p2p_mac = p2p_mac_of(&mac)?;
    let name = match tag(xml, "user-device-name") {
        n if n.is_empty() => tag(xml, "friendly-device-name"),
        n => n,
    };
    Some(Roku { ip: ip.into(), name, model: tag(xml, "model-name"), wifi_mac: mac, p2p_mac })
}

/// GET http://ip:8060/query/<what>; the body, or None on any failure.
pub fn ecp_get(ip: &str, what: &str) -> Option<String> {
    http_get(format!("{ip}:8060").parse().ok()?, &format!("/query/{what}"))
}

fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr, ECP_TIMEOUT).ok()?;
    s.set_read_timeout(Some(ECP_TIMEOUT)).ok()?;
    s.set_write_timeout(Some(ECP_TIMEOUT)).ok()?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).ok()?; // one write: some embedded HTTP servers read only the first segment
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf); // a read timeout after the body still leaves the body in buf
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.split_whitespace().nth(1).filter(|c| *c == "200")?;
    Some(body.to_string())
}

/// Scan the Wi-Fi interface's /24 for Rokus answering ECP, sorted by name. Uses the Wi-Fi iface address, NOT the
/// default route: a VPN may own the default route.
pub fn lan_rokus(prefix: Option<&str>, env: &Env) -> Result<Vec<Roku>, String> {
    let prefix = match prefix {
        Some(p) => p.to_string(),
        None => {
            let wifi = wifi_iface(env)?;
            let out = sh(&["ip", "-4", "-o", "addr", "show", "dev", &wifi], true, env)?;
            prefix_of(&out).ok_or_else(|| format!("no IPv4 address on {wifi}"))?
        }
    };
    let next = AtomicUsize::new(1);
    let found = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..64 {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i > 254 {
                    break;
                }
                let ip = format!("{prefix}{i}");
                if let Some(r) = ecp_get(&ip, "device-info").and_then(|x| parse_device_info(&ip, &x)) {
                    found.lock().unwrap().push((i, r));
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort_by_key(|(i, _)| *i); // address order first, as Python's map(), then a stable sort by name
    let mut rokus: Vec<Roku> = found.into_iter().map(|(_, r)| r).collect();
    rokus.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rokus)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "<device-info>\n<udn>x</udn>\n<model-name>Roku TV</model-name>\n\
                        <wifi-mac>4C:50:DD:21:8B:C4</wifi-mac>\n<friendly-device-name>TCL</friendly-device-name>\n\
                        <user-device-name>Living Room</user-device-name>\n</device-info>";

    #[test]
    fn test_parse_device_info() {
        let r = parse_device_info("192.168.0.63", INFO).unwrap();
        assert_eq!((r.name.as_str(), r.model.as_str()), ("Living Room", "Roku TV"));
        assert_eq!((r.wifi_mac.as_str(), r.p2p_mac.as_str()), ("4c:50:dd:21:8b:c4", "4e:50:dd:21:8b:c4"));
        let unnamed = INFO.replace("<user-device-name>Living Room</user-device-name>", "");
        assert_eq!(parse_device_info("x", &unnamed).unwrap().name, "TCL");
        assert!(parse_device_info("x", "<device-info></device-info>").is_none());
        // wired-only: no wifi MAC
    }

    #[test]
    fn test_prefix_of() {
        let out = "3: wlo1    inet 192.168.0.42/24 brd 192.168.0.255 scope global dynamic wlo1\\       valid_lft";
        assert_eq!(prefix_of(out).as_deref(), Some("192.168.0."));
        assert_eq!(prefix_of(""), None);
    }

    fn serve(answer: &'static str) -> SocketAddr {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut req = [0u8; 512];
            let n = c.read(&mut req).unwrap();
            assert!(req[..n].starts_with(b"GET /query/device-info HTTP/1.1\r\n"));
            let _ = c.write_all(answer.as_bytes());
        });
        addr
    }

    #[test]
    fn test_http_get() {
        let ok = serve("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\n\r\n<device-info><wifi-mac>4c:50:dd:21:8b:c4</wifi-mac></device-info>");
        let body = http_get(ok, "/query/device-info").unwrap();
        assert_eq!(parse_device_info("127.0.0.1", &body).unwrap().p2p_mac, "4e:50:dd:21:8b:c4");
        let missing = serve("HTTP/1.1 404 Not Found\r\n\r\nnope");
        assert_eq!(http_get(missing, "/query/device-info"), None);
        assert_eq!(ecp_get("not an ip", "device-info"), None);
    }
}
