//! The `rokast` binary as a black box: a fake GitHub on localhost counts requests.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Answers every request with a release tagged v9.9.9; returns its URL and the request counter.
fn fake_github() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repos/x/releases/latest", l.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let mut c = c.unwrap();
            let mut req = [0u8; 2048];
            let _ = c.read(&mut req);
            h.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"tag_name": "v9.9.9", "html_url": "https://example.invalid/rel"}"#;
            let _ = write!(
                c,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, hits)
}

fn rokast(home: &Path, url: &str, args: &[&str], envs: &[(&str, &str)]) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(env!("CARGO_BIN_EXE_rokast"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("ROKAST_ROOT", root)
        .env("ROKAST_CONFIG", home.join("rokast.conf"))
        .env("ROKAST_UPDATE_URL", url)
        .envs(envs.iter().copied())
        .output()
        .unwrap();
    assert!(out.status.success(), "rokast {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn test_newer_tag_shows_notice_once_a_day() {
    let t = tempfile::tempdir().unwrap();
    let (url, hits) = fake_github();
    let out = rokast(t.path(), &url, &["version", "--check"], &[]);
    assert!(out.contains("rokast 9.9.9 is available: https://example.invalid/rel"), "{out}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    rokast(t.path(), &url, &["version", "--check"], &[]); // same day: answered from the cache
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let d: serde_json::Value =
        serde_json::from_str(&rokast(t.path(), &url, &["version", "--check", "--json"], &[])).unwrap();
    assert_eq!(d["newer"], true);
    assert_eq!((d["latest"].as_str(), d["checked"].as_bool()), (Some("9.9.9"), Some(false)));
}

#[test]
fn test_check_updates_0_makes_no_request_from_the_binary() {
    let t = tempfile::tempdir().unwrap();
    let (url, hits) = fake_github();
    let out = rokast(t.path(), &url, &["version", "--check"], &[("CHECK_UPDATES", "0")]);
    assert!(out.contains("update check is off"), "{out}");
    std::fs::write(t.path().join("rokast.conf"), "CHECK_UPDATES=0\n").unwrap();
    rokast(t.path(), &url, &["version", "--check", "--json"], &[]);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
