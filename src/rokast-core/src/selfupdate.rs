//! rokast update [--check] [--yes] [--rollback] [--auto]: replace this install with the latest signed release
//!.
//!
//! Installs made by scripts/install (update::kind() == "user") replace themselves. .deb/.rpm installs get the new
//! package downloaded and checked the same way, then installed by apt-get/dnf through pkexec (the polkit password
//! prompt is the consent; as root it runs directly; without pkexec or a polkit agent it prints the command). AUR
//! installs and git clones are told how to update instead. Steps (docs/plan-v0.5.md, phase 9):
//!   1. the latest release from the GitHub API (no drafts or prereleases; only if newer than this version)
//!   2. refuse while a `rokast start` runs (update::mirroring())
//!   3. download the tarball, SHA256SUMS and SHA256SUMS.sig into $XDG_CACHE_HOME/rokast/download/<v>/
//!   4. SHA256SUMS.sig must verify against THIS install's config/release-signers, the tarball must match SHA256SUMS
//!   5. extract (every member inside rokast-<v>/, no links out) and run the new version's scripts/install, which
//!      builds the supplicant before it switches `current` (a failed build changes nothing)
//!   6. check that the new version runs; if not, scripts/install --rollback
//!
//! Progress goes to stdout as `update: <state> [detail]` lines (checking, uptodate, available, downloading, verifying,
//! installing, done, error, skipped); the app shows them. $ROKAST_UPDATE_URL (tests) also allows downloads from its
//! origin. --auto (the app runs it at start) installs only if auto_reason() is None, and is silent otherwise.
use crate::pyfmt::repr;
use crate::{paths, proc, update, Env};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use url::Url;

pub const GITHUB_HOSTS: [&str; 4] =
    ["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;

fn how(kind: &str) -> &'static str {
    match kind {
        "git" => "this is a git clone: update it with `git pull`",
        "arch" => "installed from the AUR: update it with your AUR helper (e.g. `yay -Syu rokast`)",
        _ => "not installed by scripts/install: update it the way you installed it",
    }
}

/// The release asset this kind of install updates from, or None if it can't update itself.
pub fn asset(kind: &str, v: &str) -> Option<String> {
    match kind {
        "user" => Some(format!("rokast-{v}-x86_64-linux.tar.gz")),
        "deb" => Some(format!("rokast_{v}_amd64.deb")),
        "rpm" => Some(format!("rokast-{v}-1.x86_64.rpm")),
        _ => None,
    }
}

fn pkg_tool(kind: &str) -> Option<[&'static str; 3]> {
    match kind {
        "deb" => Some(["apt-get", "install", "-y"]),
        "rpm" => Some(["dnf", "install", "-y"]),
        _ => None,
    }
}

pub fn say(state: &str, detail: &str) {
    println!("update: {state}{}", if detail.is_empty() { String::new() } else { format!(" {detail}") });
}

/// (scheme, netloc) as Python's urlsplit gives them.
fn origin(url: &str) -> Option<(String, String)> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.to_string();
    Some((u.scheme().into(), u.port().map(|p| format!("{host}:{p}")).unwrap_or(host)))
}

/// HTTPS to GitHub only (plus the test server's origin when $ROKAST_UPDATE_URL is set).
pub fn allowed(url: &str, env: &Env) -> bool {
    let Some((scheme, netloc)) = origin(url) else { return false };
    if let Some(test) = env.get("ROKAST_UPDATE_URL").filter(|v| !v.is_empty()) {
        if origin(test) == Some((scheme.clone(), netloc.clone())) {
            return true;
        }
    }
    scheme == "https" && GITHUB_HOSTS.contains(&netloc.split(':').next().unwrap_or(""))
}

pub struct Release {
    pub version: String,
    pub url: String,
    pub assets: BTreeMap<String, String>,
}

/// The network side of an update; tests use a fake.
pub trait Net {
    fn latest_release(&self, env: &Env) -> Result<Release, String>;
    fn download(&self, url: &str, dest: &Path, env: &Env) -> Result<(), String>;
}

/// GitHub over HTTPS; every redirect is checked against allowed().
pub struct Http;

fn open(url: &str, env: &Env) -> Result<ureq::Response, String> {
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(30)).build();
    let mut url = url.to_string();
    for _ in 0..10 {
        if !allowed(&url, env) {
            return Err(format!("refusing to download from {url}"));
        }
        let netloc = origin(&url).map(|o| o.1).unwrap_or_default();
        let r = agent
            .get(&url)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "rokast")
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(_, r) => format!("could not reach {netloc}: {}", r.status_text()),
                ureq::Error::Transport(t) => format!("could not reach {netloc}: {}", t.message().unwrap_or("error")),
            })?;
        if !(300..400).contains(&r.status()) {
            return Ok(r);
        }
        let to = r.header("location").unwrap_or("").to_string();
        let next = Url::parse(&url).and_then(|u| u.join(&to)).map(String::from).unwrap_or(to);
        if !allowed(&next, env) {
            return Err(format!("refusing redirect to {next}"));
        }
        url = next;
    }
    Err(format!("too many redirects from {url}"))
}

impl Net for Http {
    fn latest_release(&self, env: &Env) -> Result<Release, String> {
        let url = env.get("ROKAST_UPDATE_URL").filter(|v| !v.is_empty()).map(String::as_str).unwrap_or(update::URL);
        let d: Value = open(url, env)?.into_json().map_err(|_| "GitHub sent something that is not JSON")?;
        parse_release(&d)
    }

    fn download(&self, url: &str, dest: &Path, env: &Env) -> Result<(), String> {
        let mut r = open(url, env)?.into_reader().take(MAX_BYTES + 1);
        let mut f = File::create(dest).map_err(|e| e.to_string())?;
        let n = std::io::copy(&mut r, &mut f).map_err(|e| e.to_string())?;
        if n > MAX_BYTES {
            let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(format!("{name} is larger than {} MB", MAX_BYTES >> 20));
        }
        Ok(())
    }
}

/// The latest published, non-prerelease release from the API's JSON.
pub fn parse_release(d: &Value) -> Result<Release, String> {
    let truthy = |k: &str| d.get(k).is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)));
    let tag = d.get("tag_name").and_then(Value::as_str).unwrap_or("");
    if truthy("draft") || truthy("prerelease") || update::parse_version(tag).is_empty() {
        return Err("the latest release is not a published version".into());
    }
    let page = d.get("html_url").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(update::RELEASES);
    let assets = d.get("assets").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let assets = assets
        .iter()
        .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
        .collect();
    Ok(Release { version: tag.trim_start_matches('v').into(), url: page.into(), assets })
}

pub fn sha256(path: &Path) -> std::io::Result<String> {
    let mut h = Sha256::new();
    std::io::copy(&mut File::open(path)?, &mut h)?;
    Ok(paths::hex(&h.finalize()))
}

/// Signature first (so SHA256SUMS can be trusted), then the tarball's line in it.
pub fn verify(folder: &Path, tarball: &str, signers: &Path) -> Result<(), String> {
    let sums = folder.join("SHA256SUMS");
    if !update::verify_signature(&sums, &folder.join("SHA256SUMS.sig"), signers) {
        return Err("SHA256SUMS.sig is not a valid rokast release signature".into());
    }
    let text = fs::read_to_string(&sums).map_err(|e| e.to_string())?;
    let want: BTreeMap<&str, &str> = text
        .lines()
        .filter_map(|ln| match ln.split_whitespace().collect::<Vec<_>>()[..] {
            [h, n] => Some((n.trim_start_matches('*'), h)),
            _ => None,
        })
        .collect();
    let got = sha256(&folder.join(tarball)).map_err(|e| e.to_string())?;
    if want.get(tarball) != Some(&got.as_str()) {
        return Err(format!("{tarball} does not match SHA256SUMS"));
    }
    Ok(())
}

/// os.path.normpath for a relative path: drop '.', fold '..'.
fn normpath(p: &Path) -> String {
    let mut out: Vec<String> = vec![];
    for c in p.components() {
        match c {
            Component::ParentDir if out.last().is_some_and(|l| l != "..") => {
                out.pop();
            }
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => out.push("..".into()),
            _ => {}
        }
    }
    out.join("/")
}

fn gz(tarball: &Path) -> Result<tar::Archive<flate2::read::GzDecoder<File>>, String> {
    Ok(tar::Archive::new(flate2::read::GzDecoder::new(File::open(tarball).map_err(|e| e.to_string())?)))
}

/// Extract into folder/rokast-<version>/, refusing absolute paths, '..', links out of the tree and devices.
pub fn extract(tarball: &Path, folder: &Path, version: &str) -> Result<PathBuf, String> {
    let top = format!("rokast-{version}");
    let terr = |e: std::io::Error| e.to_string();
    let mut a = gz(tarball)?;
    for m in a.entries().map_err(terr)? {
        let m = m.map_err(terr)?;
        let name = String::from_utf8_lossy(&m.path_bytes()).trim_end_matches('/').to_string();
        let parts: Vec<&str> = name.split('/').collect();
        if name.starts_with('/') || parts.contains(&"..") || parts[0] != top {
            return Err(format!("unsafe path in the tarball: {name}"));
        }
        let t = m.header().entry_type();
        if !(t.is_file() || t.is_dir() || t.is_symlink()) {
            return Err(format!("unexpected file type in the tarball: {name}"));
        }
        if t.is_symlink() {
            let link = m.link_name_bytes().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let parent = Path::new(&name).parent().unwrap_or(Path::new(""));
            let target = normpath(&parent.join(&link));
            if link.starts_with('/') || !target.starts_with(&format!("{top}/")) {
                return Err(format!("link out of the tree in the tarball: {name}"));
            }
        }
    }
    fs::create_dir_all(folder).map_err(terr)?;
    gz(tarball)?.unpack(folder).map_err(terr)?;
    Ok(folder.join(top))
}

/// (exit code, stdout + stderr) of a command run with exactly `env`.
fn capture(cmd: &mut Command, env: &Env) -> std::io::Result<(i32, String)> {
    let o = cmd.env_clear().envs(env).stdin(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
    Ok((proc::returncode(&o.status), text))
}

/// What `<exe> version` prints (stripped), or "" if it fails or takes over a minute.
fn version_of(exe: &Path, env: &Env) -> String {
    let child = Command::new(exe).arg("version").env_clear().envs(env).stdout(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return String::new() };
    let Some(st) = proc::wait_for(&mut child, Duration::from_secs(60)) else { return String::new() };
    let mut out = String::new();
    let _ = child.stdout.take().map(|mut s| s.read_to_string(&mut out));
    if st.code().is_none() {
        return String::new();
    }
    out.trim().to_string()
}

fn log_lines(text: &str, last: Option<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let from = last.map(|n| lines.len().saturating_sub(n)).unwrap_or(0);
    for ln in &lines[from..] {
        println!("update: log {ln}");
    }
}

#[allow(clippy::too_many_arguments)]
/// Returns the exit code. See the module docs for the steps. `kind` is update::kind() of `root`.
pub fn run_update(
    net: &dyn Net,
    root: &Path,
    kind: &str,
    current: &str,
    yes: bool,
    check_only: bool,
    env: &Env,
    ask: &dyn Fn(&str) -> String,
) -> Result<i32, String> {
    say("checking", "");
    let rel = net.latest_release(env)?;
    if !update::is_newer(&rel.version, current) {
        say("uptodate", current);
        return Ok(0);
    }
    say("available", &rel.version);
    let v = rel.version.as_str();
    let Some(tarball) = asset(kind, v) else {
        say("error", how(kind));
        return Ok(1);
    };
    if check_only {
        return Ok(0);
    }
    if update::mirroring(env) {
        say("error", "stop mirroring first (a rokast start is running)");
        return Ok(1);
    }
    if proc::which("ssh-keygen", env).is_none() {
        say(
            "error",
            "checking the release signature needs ssh-keygen: install openssh (openssh-client on Debian/Ubuntu)",
        );
        return Ok(1);
    }
    if !yes
        && (!std::io::stdin().is_terminal()
            || ask(&format!("Update rokast {current} -> {v}? [y/N] ")).trim().to_lowercase() != "y")
    {
        say("error", "not confirmed (use --yes)");
        return Ok(1);
    }
    let names = [tarball.as_str(), "SHA256SUMS", "SHA256SUMS.sig"];
    let missing: Vec<&str> = names.iter().copied().filter(|n| !rel.assets.contains_key(*n)).collect();
    if !missing.is_empty() {
        say("error", &format!("release {v} has no {} (unsigned releases are not installed)", missing.join(", ")));
        return Ok(1);
    }
    let downloads = update::cache_file(env).parent().map(|d| d.join("download")).unwrap_or_default();
    let folder = downloads.join(v);
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let res = (|| -> Result<i32, String> {
        for name in ["SHA256SUMS", "SHA256SUMS.sig", tarball.as_str()] {
            say("downloading", name);
            net.download(&rel.assets[name], &folder.join(name), env)?;
        }
        say("verifying", "");
        verify(&folder, &tarball, &root.join("config").join("release-signers"))?;
        say("installing", v);
        if let Some(tool) = pkg_tool(kind) {
            return install_package(&folder.join(&tarball), v, &tool, env);
        }
        let new = extract(&folder.join(&tarball), &folder.join("x"), v)?;
        install(&new, v, env)
    })();
    let _ = fs::remove_dir_all(&folder);
    let _ = fs::remove_dir(&downloads); // download/, if nothing else is in it
    res.or_else(|e| {
        say("error", &e);
        Ok(1)
    })
}

fn install(new: &Path, v: &str, env: &Env) -> Result<i32, String> {
    let base = paths::data_dir(env);
    let current = base.join("current");
    let before = update::realpath(&current);
    let (code, text) =
        capture(&mut Command::new(new.join("scripts").join("install")), env).map_err(|e| e.to_string())?;
    log_lines(&text, None);
    if code != 0 {
        return Err("the new version's installer failed; nothing changed".into());
    }
    let now = update::realpath(&current);
    let got = version_of(&current.join("scripts").join("rokast"), env);
    if got != v {
        if now != before {
            let _ = capture(Command::new(current.join("scripts").join("install")).arg("--rollback"), env);
        }
        return Err(format!("the new version does not run (version said {}); rolled back", repr(&got)));
    }
    say("done", v);
    Ok(0)
}

/// apt-get/dnf install the verified package: directly as root, else through pkexec. If that can't ask for the
/// password (no pkexec, no polkit agent, dialog dismissed), keep the package and print the command instead.
fn install_package(pkg: &Path, v: &str, tool: &[&str; 3], env: &Env) -> Result<i32, String> {
    let mut cmd: Vec<String> = tool.iter().map(|s| s.to_string()).collect();
    cmd.push(pkg.to_string_lossy().into_owned());
    // SAFETY: geteuid has no preconditions
    let cmd = if unsafe { libc::geteuid() } == 0 {
        Some(cmd)
    } else if proc::which("pkexec", env).is_some() {
        Some([vec!["pkexec".to_string()], cmd].concat())
    } else {
        None
    };
    let ran = match &cmd {
        Some(c) => Some(capture(Command::new(&c[0]).args(&c[1..]), env).map_err(|e| e.to_string())?),
        None => None,
    };
    let no_prompt = match (&cmd, &ran) {
        (Some(c), Some((code, _))) => c[0] == "pkexec" && matches!(code, 126 | 127),
        _ => true,
    };
    if no_prompt {
        let dir = update::cache_file(env).parent().map(Path::to_path_buf).unwrap_or_default();
        let keep = dir.join(pkg.file_name().unwrap_or_default());
        if fs::rename(pkg, &keep).is_err() {
            fs::copy(pkg, &keep).map_err(|e| e.to_string())?;
        }
        return Err(format!(
            "could not ask for your password; install it yourself: sudo {} {}",
            tool.join(" "),
            keep.display()
        ));
    }
    let (code, text) = ran.unwrap_or_default();
    log_lines(&text, Some(20));
    if code != 0 {
        return Err(format!("{} failed (exit {code})", tool[0]));
    }
    let got = proc::which("rokast", env).map(|exe| version_of(&exe, env)).unwrap_or_default();
    if got != v {
        return Err(format!("{} finished but rokast version says {}", tool[0], repr(&got)));
    }
    say("done", v);
    Ok(0)
}

pub fn run_rollback(env: &Env) -> i32 {
    if update::mirroring(env) {
        say("error", "stop mirroring first (a rokast start is running)");
        return 1;
    }
    let script = paths::data_dir(env).join("current").join("scripts").join("install");
    if !script.exists() {
        say("error", "not installed by scripts/install: nothing to roll back");
        return 1;
    }
    match capture(Command::new(&script).arg("--rollback"), env) {
        Ok((code, text)) => {
            println!("{}", text.trim());
            code
        }
        Err(e) => {
            say("error", &e.to_string());
            1
        }
    }
}

/// Why `update --auto` does nothing, or None if it may install: AUTO_UPDATE=1 (default off), update checks on, an
/// install made by scripts/install, and nothing mirroring.
pub fn auto_reason(conf: &BTreeMap<String, String>, kind: &str, mirroring: bool) -> Option<String> {
    let get = |k: &str, d: &'static str| conf.get(k).map(String::as_str).unwrap_or(d).to_string();
    if get("AUTO_UPDATE", "0") != "1" {
        return Some("AUTO_UPDATE is off".into());
    }
    if get("CHECK_UPDATES", "1") == "0" {
        return Some("CHECK_UPDATES=0".into());
    }
    if kind != "user" {
        return Some(format!("{kind} install"));
    }
    mirroring.then(|| "mirroring".into())
}

fn ask_stdin(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s
}

/// `rokast update <args>`: the exit code.
pub fn main(root: &Path, current: &str, args: &[String], conf: &BTreeMap<String, String>, env: &Env) -> i32 {
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--rollback") {
        return run_rollback(env);
    }
    let kind = update::kind(root, env, update::owns);
    let mut yes = has("--yes");
    if has("--auto") {
        if let Some(why) = auto_reason(conf, kind, update::mirroring(env)) {
            say("skipped", &why);
            return 0;
        }
        yes = true;
    }
    run_update(&Http, root, kind, current, yes, has("--check"), env, &ask_stdin).unwrap_or_else(|e| {
        say("error", &e);
        1
    })
}

#[cfg(test)]
#[path = "selfupdate_tests.rs"]
mod tests;
