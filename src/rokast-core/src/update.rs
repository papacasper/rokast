//! Update notice: is a newer rokast release out? Never downloads anything (src/miracast/update.py).
//!
//! At most one request a day to GitHub's latest-release API; the answer is cached in $XDG_CACHE_HOME/rokast/update.json
//! (the attempt time is stored even when offline, so a dead network is not retried on every start).
//! kind() says how this copy was installed; mirroring() whether a `rokast start` is running (its session lock);
//! verify_signature() checks a release file against config/release-signers.
use crate::Env;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::process::{Command, Stdio};

pub const URL: &str = "https://api.github.com/repos/papacasper/rokast/releases/latest";
pub const RELEASES: &str = "https://github.com/papacasper/rokast/releases/latest";
pub const DAY: f64 = 24.0 * 3600.0;

/// 'v0.10.2' -> [0, 10, 2]; anything unparsable -> [] (never "newer").
pub fn parse_version(v: &str) -> Vec<i64> {
    let core = v.trim().trim_start_matches('v').split('-').next().unwrap_or("");
    core.split('.').map(|x| x.parse::<i64>()).collect::<Result<Vec<_>, _>>().unwrap_or_default()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (parse_version(latest), parse_version(current));
    !a.is_empty() && !b.is_empty() && a > b
}

fn home(env: &Env) -> String {
    env.get("HOME").filter(|h| !h.is_empty()).cloned().unwrap_or_else(|| "/".into())
}

pub fn cache_file(env: &Env) -> PathBuf {
    let base = env.get("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    base.unwrap_or_else(|| Path::new(&home(env)).join(".cache")).join("rokast").join("update.json")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub current: String,
    pub latest: Option<String>,
    pub newer: bool,
    pub url: String,
    pub checked: bool,
    pub enabled: bool,
}

/// Makes a request only when enabled and the cache is a day old. `fetcher(url)` -> (tag, page url) or None.
pub fn check(
    current: &str,
    enabled: bool,
    env: &Env,
    now: f64,
    fetcher: impl Fn(&str) -> Option<(String, String)>,
) -> Check {
    let mut out =
        Check { current: current.into(), latest: None, newer: false, url: RELEASES.into(), checked: false, enabled };
    if !enabled {
        return out;
    }
    let path = cache_file(env);
    let mut c: Value = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    if !c.is_object() {
        c = json!({});
    }
    let fresh = c.get("at").and_then(Value::as_f64).is_some_and(|at| (0.0..DAY).contains(&(now - at)));
    if !fresh {
        let url = env.get("ROKAST_UPDATE_URL").filter(|v| !v.is_empty()).map(String::as_str).unwrap_or(URL);
        let got = fetcher(url);
        let (latest, page) = match got {
            Some((t, u)) => (json!(t), json!(u)),
            None => (c.get("latest").cloned().unwrap_or(Value::Null), c.get("url").cloned().unwrap_or(Value::Null)),
        };
        c = json!({"at": now, "latest": latest, "url": page});
        out.checked = true;
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir).and_then(|_| fs::write(&path, c.to_string()));
        }
    }
    if let Some(latest) = c.get("latest").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        out.latest = Some(latest.trim_start_matches('v').into());
        out.url = c.get("url").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(RELEASES).into();
        out.newer = is_newer(latest, current);
    }
    out
}

/// (tag, page url) of the latest release, or None on any failure (offline, rate-limited, bad JSON: no notice, try
/// again tomorrow).
pub fn fetch(url: &str) -> Option<(String, String)> {
    let d: Value = ureq::get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "rokast")
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .ok()?
        .into_json()
        .ok()?;
    let tag = d.get("tag_name")?.as_str()?.to_string();
    let page = d.get("html_url").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(RELEASES);
    Some((tag, page.into()))
}

pub fn packaged(root: &str) -> bool {
    root.starts_with("/usr/")
}

/// `os.path.realpath`: resolve what exists, keep the rest as given.
pub fn realpath(p: &Path) -> PathBuf {
    if let Ok(r) = fs::canonicalize(p) {
        return r;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => realpath(parent).join(name),
        _ => p.to_path_buf(),
    }
}

/// True if `cmd path` exits 0 (a package manager owns the path).
pub fn owns(cmd: &[&str], path: &Path) -> bool {
    Command::new(cmd[0])
        .args(&cmd[1..])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// How this copy was installed: 'user' (scripts/install, under $XDG_DATA_HOME/rokast/versions/), 'deb', 'rpm',
/// 'arch' (a package owns the folder), 'git' (a clone), or 'other'. Only 'user' may replace itself.
pub fn kind(root: &Path, env: &Env, owns: impl Fn(&[&str], &Path) -> bool) -> &'static str {
    let real = realpath(root);
    let data = env.get("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let data = data.unwrap_or_else(|| Path::new(&home(env)).join(".local").join("share"));
    let versions = format!("{}{MAIN_SEPARATOR}", realpath(&data.join("rokast").join("versions")).display());
    if real.to_string_lossy().starts_with(&versions) {
        return "user";
    }
    if real.join(".git").exists() {
        return "git";
    }
    let tools: [(&str, &[&str]); 3] =
        [("deb", &["dpkg", "-S"]), ("rpm", &["rpm", "-qf"]), ("arch", &["pacman", "-Qo"])];
    tools.iter().find(|(_, cmd)| owns(cmd, &real)).map(|(name, _)| *name).unwrap_or("other")
}

/// The session lock `rokast start` holds while it runs.
pub fn lock_file(env: &Env) -> PathBuf {
    let base = match env.get("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => cache_file(env).parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default(),
    };
    base.join("rokast").join("session.lock")
}

fn flock(f: &File, op: libc::c_int) -> bool {
    unsafe { libc::flock(f.as_raw_fd(), op | libc::LOCK_NB) == 0 }
}

/// Take the session lock; keep the returned file open for as long as the session runs.
pub fn hold_session(env: &Env) -> std::io::Result<File> {
    let path = lock_file(env);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let f = OpenOptions::new().append(true).create(true).open(&path)?;
    // another session runs: it fails on its own (one P2P link) and the lock stays held either way
    let _ = flock(&f, libc::LOCK_EX);
    Ok(f)
}

/// True while some `rokast start` holds the session lock.
pub fn mirroring(env: &Env) -> bool {
    match File::open(lock_file(env)) {
        Ok(f) => !flock(&f, libc::LOCK_SH),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// True if `sig` is a valid rokast-release signature of `path` by a key in `signers` (ssh-keygen -Y verify).
pub fn verify_signature(path: &Path, sig: &Path, signers: &Path) -> bool {
    let Ok(input) = File::open(path) else {
        return false;
    };
    Command::new("ssh-keygen")
        .args(["-q", "-Y", "verify", "-f"])
        .arg(signers)
        .args(["-I", "rokast-release", "-n", "rokast-release", "-s"])
        .arg(sig)
        .stdin(input)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// One line for people, or "" when there is nothing to say.
pub fn message(res: &Check, root: &str) -> String {
    if !res.newer {
        return String::new();
    }
    let how = if packaged(root) { "update with your package manager" } else { res.url.as_str() };
    format!("rokast {} is available: {how}", res.latest.as_deref().unwrap_or("None"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn env_in(t: &tempfile::TempDir) -> Env {
        let h = t.path().to_str().unwrap().to_string();
        [("HOME".to_string(), h.clone()), ("XDG_RUNTIME_DIR".to_string(), format!("{h}/run"))].into()
    }

    #[test]
    fn test_versions() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.2.0"));
        assert_eq!(parse_version("v1.2.3-rc1"), [1, 2, 3]);
    }

    #[test]
    fn test_cache_expires_after_a_day() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        let hits = RefCell::new(0);
        let tag = RefCell::new("v9.9.9");
        let fetch = |_: &str| {
            *hits.borrow_mut() += 1;
            Some((tag.borrow().to_string(), "https://example.invalid/rel".into()))
        };
        assert!(check("0.2.0", true, &env, 1000.0, fetch).newer);
        check("0.2.0", true, &env, 1000.0 + DAY - 1.0, fetch);
        assert_eq!(*hits.borrow(), 1);
        *tag.borrow_mut() = "v0.2.0";
        let res = check("0.2.0", true, &env, 1000.0 + DAY, fetch);
        assert_eq!(*hits.borrow(), 2);
        assert!(!res.newer);
    }

    #[test]
    fn test_check_updates_0_makes_no_request() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        let res = check("0.2.0", false, &env, 1.0, |_| panic!("no request when off"));
        assert!(!res.enabled && !res.checked);
        assert!(!cache_file(&env).exists());
    }

    #[test]
    fn test_offline_is_quiet_and_not_retried_today() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        let res = check("0.2.0", true, &env, 5000.0, |_| None);
        assert_eq!((res.newer, res.checked), (false, true));
        assert!(!check("0.2.0", true, &env, 5001.0, |_| None).checked);
    }

    #[test]
    fn test_packaged_install_points_at_package_manager() {
        let mut res = Check {
            current: "0.1.0".into(),
            latest: Some("9.9.9".into()),
            newer: true,
            url: "u".into(),
            checked: false,
            enabled: true,
        };
        assert_eq!(message(&res, "/usr/lib/rokast"), "rokast 9.9.9 is available: update with your package manager");
        assert_eq!(message(&res, "/home/u/.local/share/rokast/current"), "rokast 9.9.9 is available: u");
        res.newer = false;
        assert_eq!(message(&res, "/x"), "");
    }

    #[test]
    fn test_kind() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        let user = t.path().join(".local/share/rokast/versions/0.5.0");
        let clone = t.path().join("src/rokast");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(clone.join(".git")).unwrap();
        let link = t.path().join(".local/share/rokast/current");
        std::os::unix::fs::symlink(&user, &link).unwrap();
        let never = |_: &[&str], _: &Path| false;
        assert_eq!(kind(&user, &env, never), "user");
        assert_eq!(kind(&link, &env, never), "user"); // through the current/ link
        assert_eq!(kind(&clone, &env, never), "git");
        assert_eq!(kind(Path::new("/usr/lib/rokast"), &env, never), "other");
        for (name, tool) in [("deb", "dpkg"), ("rpm", "rpm"), ("arch", "pacman")] {
            assert_eq!(kind(Path::new("/usr/lib/rokast"), &env, |cmd: &[&str], _: &Path| cmd[0] == tool), name);
        }
    }

    #[test]
    fn test_session_lock() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        assert!(!mirroring(&env)); // no lock file yet
                                   // flock locks belong to the open file description, so a second open in this process sees the hold
        let held = hold_session(&env).unwrap();
        assert!(mirroring(&env));
        drop(held);
        assert!(!mirroring(&env)); // the lock dies with the file
    }
}
