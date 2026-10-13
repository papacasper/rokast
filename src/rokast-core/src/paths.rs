//! Where rokast keeps things, independent of where it is installed (src/miracast/paths.py).
//!
//!   config      $ROKAST_CONFIG, else $XDG_CONFIG_HOME/rokast/rokast.conf   (legacy: <install>/config/rokast.conf)
//!   supplicant  $ROKAST_SUPPLICANT, else $XDG_DATA_HOME/rokast/supplicant/wpa_supplicant, else the packaged
//!               /usr/lib/rokast/wpa_supplicant                              (legacy: <install>/vendor/hostap-src/...)
//!
//! scripts/build-supplicant and ui/src/config.rs follow the same rules; keep them in step.
use crate::Env;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const PACKAGED_SUPPLICANT: &str = "/usr/lib/rokast/wpa_supplicant";
pub const PATCH_FILES: [&str; 2] = ["patches/hostap-2.11-roku.patch", "patches/hostap.config"];

fn home(env: &Env) -> PathBuf {
    PathBuf::from(env.get("HOME").filter(|h| !h.is_empty()).cloned().unwrap_or_else(|| "/".into()))
}

/// `$VAR`, else `$HOME/<fallback>`.
pub fn xdg(env: &Env, var: &str, fallback: &str) -> PathBuf {
    match env.get(var).filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home(env).join(fallback),
    }
}

pub fn config_dir(env: &Env) -> PathBuf {
    xdg(env, "XDG_CONFIG_HOME", ".config").join("rokast")
}

pub fn data_dir(env: &Env) -> PathBuf {
    xdg(env, "XDG_DATA_HOME", ".local/share").join("rokast")
}

pub fn cache_dir(env: &Env) -> PathBuf {
    xdg(env, "XDG_CACHE_HOME", ".cache").join("rokast")
}

pub fn legacy_config(root: &Path) -> PathBuf {
    root.join("config").join("rokast.conf")
}

/// (file to read, legacy file to migrate into it).
pub fn config_file(env: &Env, root: &Path) -> (PathBuf, Option<PathBuf>) {
    if let Some(p) = env.get("ROKAST_CONFIG").filter(|v| !v.is_empty()) {
        return (PathBuf::from(p), None);
    }
    let path = config_dir(env).join("rokast.conf");
    let legacy = legacy_config(root);
    let migrate = !path.exists() && legacy.is_file();
    (path, migrate.then_some(legacy))
}

/// Copy the pre-0.2 in-tree config to its new home; rename the old one so nobody edits a dead file.
pub fn migrate_config(path: &Path, legacy: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::copy(legacy, path)?;
    let mut moved = legacy.as_os_str().to_owned();
    moved.push(".moved");
    fs::rename(legacy, moved)?;
    eprintln!("config: moved {} -> {}", legacy.display(), path.display());
    Ok(())
}

pub fn supplicant_candidates(env: &Env, root: &Path) -> Vec<PathBuf> {
    if let Some(p) = env.get("ROKAST_SUPPLICANT").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(p)];
    }
    vec![
        data_dir(env).join("supplicant").join("wpa_supplicant"),
        PathBuf::from(PACKAGED_SUPPLICANT),
        root.join("vendor/hostap-src/wpa_supplicant/wpa_supplicant"),
    ]
}

pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap_or_default();
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// First usable patched wpa_supplicant.
pub fn find_supplicant(env: &Env, root: &Path, usable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    supplicant_candidates(env, root).into_iter().find(|p| usable(p))
}

/// sha256 of the shipped patch + build config; scripts/build-supplicant stores it as patch.sha256.
pub fn patch_hash(root: &Path) -> std::io::Result<String> {
    let mut h = Sha256::new();
    for f in PATCH_FILES {
        h.update(fs::read(root.join(f))?);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// "missing" | "ok" | "outdated" (built from a different patch) | "legacy" (old in-tree location) | "external".
pub fn supplicant_state(path: Option<&Path>, root: &Path) -> &'static str {
    let Some(path) = path else { return "missing" };
    if path.starts_with(root.join("vendor")) {
        return "legacy";
    }
    let stamp = path.parent().unwrap_or(Path::new("/")).join("patch.sha256");
    match fs::read_to_string(&stamp) {
        Err(_) => "external", // packaged or $ROKAST_SUPPLICANT: managed elsewhere
        Ok(s) if Some(s.trim().to_string()) == patch_hash(root).ok() => "ok",
        Ok(_) => "outdated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> Env {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn setup() -> (tempfile::TempDir, Env, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let root = t.path().join("install");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        let env = env_of(&[("HOME", home.to_str().unwrap())]);
        (t, env, root)
    }

    #[test]
    fn test_fresh_install_uses_xdg_path_and_nothing_to_migrate() {
        let (_t, env, root) = setup();
        let (path, legacy) = config_file(&env, &root);
        assert_eq!(path, home(&env).join(".config/rokast/rokast.conf"));
        assert_eq!(legacy, None);
    }

    #[test]
    fn test_xdg_config_home_is_honoured() {
        let (_t, mut env, root) = setup();
        env.insert("XDG_CONFIG_HOME".into(), "/x/cfg".into());
        assert_eq!(config_file(&env, &root).0, PathBuf::from("/x/cfg/rokast/rokast.conf"));
    }

    #[test]
    fn test_legacy_only_is_migrated_once() {
        let (_t, env, root) = setup();
        fs::write(legacy_config(&root), "TARGET=Living Room\n").unwrap();
        let (path, legacy) = config_file(&env, &root);
        let legacy = legacy.expect("legacy to migrate");
        migrate_config(&path, &legacy).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "TARGET=Living Room\n");
        assert!(!legacy.exists());
        assert!(root.join("config/rokast.conf.moved").exists());
        assert_eq!(config_file(&env, &root).1, None);
    }

    #[test]
    fn test_new_location_wins_over_legacy() {
        let (_t, env, root) = setup();
        fs::write(legacy_config(&root), "TARGET=old\n").unwrap();
        let path = config_dir(&env).join("rokast.conf");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "TARGET=new\n").unwrap();
        assert_eq!(config_file(&env, &root), (path, None));
    }

    #[test]
    fn test_rokast_config_env_overrides_and_never_migrates() {
        let (_t, mut env, root) = setup();
        fs::write(legacy_config(&root), "TARGET=old\n").unwrap();
        env.insert("ROKAST_CONFIG".into(), "/elsewhere.conf".into());
        assert_eq!(config_file(&env, &root), (PathBuf::from("/elsewhere.conf"), None));
    }

    #[test]
    fn test_lookup_order() {
        let (_t, mut env, root) = setup();
        let c = supplicant_candidates(&env, &root);
        assert_eq!(c[0], home(&env).join(".local/share/rokast/supplicant/wpa_supplicant"));
        assert_eq!(c[1], PathBuf::from(PACKAGED_SUPPLICANT));
        assert_eq!(c[2], root.join("vendor/hostap-src/wpa_supplicant/wpa_supplicant"));
        env.insert("ROKAST_SUPPLICANT".into(), "/opt/w".into());
        assert_eq!(supplicant_candidates(&env, &root), vec![PathBuf::from("/opt/w")]);
    }

    #[test]
    fn test_find_picks_first_usable() {
        let (_t, env, root) = setup();
        let got = find_supplicant(&env, &root, |p| p == Path::new(PACKAGED_SUPPLICANT));
        assert_eq!(got, Some(PathBuf::from(PACKAGED_SUPPLICANT)));
        assert_eq!(find_supplicant(&env, &root, |_| false), None);
    }

    #[test]
    fn test_state_tracks_patch_hash() {
        let (_t, env, root) = setup();
        fs::create_dir_all(root.join("patches")).unwrap();
        fs::write(root.join(PATCH_FILES[0]), "patch").unwrap();
        fs::write(root.join(PATCH_FILES[1]), "config").unwrap();
        let sup = data_dir(&env).join("supplicant/wpa_supplicant");
        fs::create_dir_all(sup.parent().unwrap()).unwrap();
        assert_eq!(supplicant_state(None, &root), "missing");
        assert_eq!(supplicant_state(Some(&sup), &root), "external");
        fs::write(sup.with_file_name("patch.sha256"), patch_hash(&root).unwrap() + "\n").unwrap();
        assert_eq!(supplicant_state(Some(&sup), &root), "ok");
        fs::write(root.join(PATCH_FILES[0]), "patch v2").unwrap();
        assert_eq!(supplicant_state(Some(&sup), &root), "outdated");
        assert_eq!(supplicant_state(Some(&root.join("vendor/hostap-src/w")), &root), "legacy");
    }

    #[test]
    fn test_patch_hash_matches_build_script() {
        // the repo's own patch files, hashed the way scripts/build-supplicant does (cat both | sha256sum)
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cat = Vec::new();
        for f in PATCH_FILES {
            cat.extend(fs::read(repo.join(f)).unwrap());
        }
        let out = std::process::Command::new("sha256sum")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(&cat)?;
                c.wait_with_output()
            })
            .unwrap();
        let want = String::from_utf8(out.stdout).unwrap();
        assert_eq!(patch_hash(&repo).unwrap(), want.split_whitespace().next().unwrap());
    }
}
