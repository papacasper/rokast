use super::*;
use std::cell::Cell;

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "rokast-selfupdate-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// A .tar.gz from (name, kind, data-or-link-target), names written raw so bad ones get through.
fn make_tar(path: &Path, members: &[(&str, &str, &str)]) {
    let gz = flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for (name, kind, data) in members {
        let mut h = tar::Header::new_gnu();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_mode(0o644);
        h.set_size(0);
        let body: &[u8] = match *kind {
            "dir" => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                b""
            }
            "file" => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_size(data.len() as u64);
                data.as_bytes()
            }
            "fifo" => {
                h.set_entry_type(tar::EntryType::Fifo);
                b""
            }
            link => {
                h.set_entry_type(if link == "sym" { tar::EntryType::Symlink } else { tar::EntryType::Link });
                h.as_old_mut().linkname[..data.len()].copy_from_slice(data.as_bytes());
                b""
            }
        };
        h.set_cksum();
        b.append(&h, body).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap();
}

fn extract_members(d: &Path, members: &[(&str, &str, &str)]) -> Result<PathBuf, String> {
    let tb = d.join("r.tar.gz");
    make_tar(&tb, members);
    extract(&tb, &d.join("x"), "1.0.0")
}

#[test]
fn test_allowed_hosts() {
    let e = Env::new();
    assert!(allowed("https://github.com/papacasper/rokast/releases/download/v1/x", &e));
    assert!(allowed("https://objects.githubusercontent.com/x", &e));
    assert!(!allowed("http://github.com/x", &e)); // not HTTPS
    assert!(!allowed("https://github.com.evil.example/x", &e));
    assert!(!allowed("http://127.0.0.1:8000/x", &e));
    let e = env(&[("ROKAST_UPDATE_URL", "http://127.0.0.1:8000/latest.json")]); // the test server's origin only
    assert!(allowed("http://127.0.0.1:8000/x", &e));
    assert!(!allowed("http://127.0.0.1:8001/x", &e));
}

#[test]
fn test_good_tarball() {
    let d = tmp();
    let out = extract_members(
        &d,
        &[
            ("rokast-1.0.0", "dir", ""),
            ("rokast-1.0.0/VERSION", "file", "1.0.0\n"),
            ("rokast-1.0.0/link", "sym", "VERSION"),
        ],
    )
    .unwrap();
    assert_eq!(fs::read_to_string(out.join("VERSION")).unwrap(), "1.0.0\n");
    assert_eq!(fs::read_to_string(out.join("link")).unwrap(), "1.0.0\n");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn test_unsafe_tarballs_are_refused() {
    let d = tmp();
    let cases: [(&str, &str, &str, &str); 7] = [
        ("rokast-1.0.0/../evil", "file", "x", "unsafe path"),
        ("/etc/evil", "file", "x", "unsafe path"),
        ("other-1.0.0/VERSION", "file", "x", "unsafe path"),
        ("rokast-1.0.0/l", "sym", "../../outside", "link out of the tree"),
        ("rokast-1.0.0/l", "sym", "/etc/passwd", "link out of the tree"),
        ("rokast-1.0.0/h", "hard", "rokast-1.0.0/VERSION", "unexpected file type"),
        ("rokast-1.0.0/p", "fifo", "", "unexpected file type"),
    ];
    for (name, kind, data, why) in cases {
        let err = extract_members(&d, &[(name, kind, data)]).unwrap_err();
        assert!(err.contains(why) && err.ends_with(name), "{name}: {err}");
    }
    assert!(!d.join("evil").exists() && !d.join("x").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn test_auto_update_decision() {
    let c = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> { env(pairs) };
    let on = c(&[("AUTO_UPDATE", "1")]);
    assert_eq!(auto_reason(&on, "user", false), None);
    assert_eq!(auto_reason(&c(&[]), "user", false).unwrap(), "AUTO_UPDATE is off"); // default off
    assert_eq!(auto_reason(&c(&[("AUTO_UPDATE", "0")]), "user", false).unwrap(), "AUTO_UPDATE is off");
    assert_eq!(
        auto_reason(&c(&[("AUTO_UPDATE", "1"), ("CHECK_UPDATES", "0")]), "user", false).unwrap(),
        "CHECK_UPDATES=0"
    );
    assert_eq!(auto_reason(&on, "user", true).unwrap(), "mirroring");
    for kind in ["deb", "rpm", "arch", "git", "other"] {
        // never for packages
        assert_eq!(auto_reason(&on, kind, false).unwrap(), format!("{kind} install"));
        assert_eq!(auto_reason(&on, kind, true).unwrap(), format!("{kind} install"));
    }
}

struct Fake {
    downloads: Cell<u32>,
}

impl Net for Fake {
    fn latest_release(&self, _: &Env) -> Result<Release, String> {
        Ok(Release { version: "9.9.9".into(), url: String::new(), assets: BTreeMap::new() })
    }
    fn download(&self, _: &str, _: &Path, _: &Env) -> Result<(), String> {
        self.downloads.set(self.downloads.get() + 1);
        Ok(())
    }
}

#[test]
fn test_no_ssh_keygen_refuses_before_downloading() {
    let d = tmp();
    let s = d.to_string_lossy();
    let e = env(&[("PATH", &s), ("XDG_CACHE_HOME", &s), ("XDG_RUNTIME_DIR", &s)]);
    let net = Fake { downloads: Cell::new(0) };
    let code = run_update(&net, &d, "user", "0.1.0", true, false, &e, &|_| "y".into()).unwrap();
    assert_eq!(code, 1);
    assert_eq!(net.downloads.get(), 0);
    assert!(!d.join("rokast").join("download").exists());
    fs::remove_dir_all(&d).unwrap();
}

// Release signing: scripts/sign-release (used by release.yml) and update::verify_signature, with a throwaway key
// (tests/test_signing.py).
struct Keys {
    d: PathBuf,
}

impl Keys {
    fn new() -> Keys {
        let d = tmp().join("keys");
        fs::create_dir_all(&d).unwrap();
        for name in ["good", "other"] {
            let st =
                Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(d.join(name)).status();
            assert!(st.unwrap().success());
        }
        let pubkey = fs::read_to_string(d.join("good.pub")).unwrap();
        let pubkey: Vec<&str> = pubkey.split_whitespace().take(2).collect();
        fs::write(d.join("signers"), format!("rokast-release namespaces=\"rokast-release\" {}\n", pubkey.join(" ")))
            .unwrap();
        fs::write(d.join("SHA256SUMS"), format!("{}  rokast-9.9.9-x86_64-linux.tar.gz\n", "0".repeat(64))).unwrap();
        Keys { d }
    }

    fn p(&self, name: &str) -> PathBuf {
        self.d.join(name)
    }

    fn sign(&self, key: Option<&str>) -> std::process::Output {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/sign-release");
        let mut cmd = Command::new(script);
        cmd.arg(self.p("SHA256SUMS")).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
        if let Some(key) = key {
            cmd.env("ROKAST_SIGNING_KEY", fs::read_to_string(self.p(key)).unwrap())
                .env("ROKAST_SIGNERS", self.p("signers"));
        }
        cmd.output().unwrap()
    }

    fn verifies(&self, sig: &str) -> bool {
        update::verify_signature(&self.p("SHA256SUMS"), &self.p(sig), &self.p("signers"))
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.d.parent().unwrap());
    }
}

#[test]
fn test_sign_and_verify() {
    let k = Keys::new();
    assert!(k.sign(Some("good")).status.success());
    assert!(k.verifies("SHA256SUMS.sig"));
}

#[test]
fn test_one_changed_byte_is_rejected() {
    let k = Keys::new();
    k.sign(Some("good"));
    let mut b = fs::read(k.p("SHA256SUMS")).unwrap();
    b[0] = b'1';
    fs::write(k.p("SHA256SUMS"), b).unwrap();
    assert!(!k.verifies("SHA256SUMS.sig"));
}

#[test]
fn test_unlisted_key_fails_to_sign_and_verify() {
    let k = Keys::new();
    let r = k.sign(Some("other"));
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("not in"));
    assert!(!k.verifies("SHA256SUMS.sig"));
}

#[test]
fn test_missing_signature_or_key() {
    let k = Keys::new();
    assert!(!k.verifies("nope.sig"));
    assert!(!k.sign(None).status.success());
}

#[test]
fn test_shipped_signers_file_is_valid() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/release-signers");
    let text = fs::read_to_string(path).unwrap();
    let lines: Vec<Vec<&str>> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert!(!lines.is_empty());
    for ln in lines {
        assert_eq!(ln[..3], ["rokast-release", "namespaces=\"rokast-release\"", "ssh-ed25519"]);
    }
}
