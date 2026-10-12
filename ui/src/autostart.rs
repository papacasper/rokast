//! "Start with my computer, in the tray": an XDG autostart entry that runs this app with `--tray`.
//! Off by default; the file existing *is* the setting. `scripts/install --uninstall` removes it.

use std::path::{Component, Path, PathBuf};

pub fn path() -> PathBuf {
    crate::config::config_home().join("autostart/io.github.papacasper.rokast.desktop")
}

pub fn enabled() -> bool {
    path().is_file()
}

/// An installed copy runs from ~/.local/share/rokast/versions/<v>/… (the kernel resolves the `current` link), but
/// that folder is deleted two upgrades later: point autostart at …/rokast/current/… so it follows upgrades.
pub fn exec_path(exe: &Path) -> PathBuf {
    let parts: Vec<Component> = exe.components().collect();
    for i in 1..parts.len().saturating_sub(2) {
        if parts[i].as_os_str() == "versions" && parts[i - 1].as_os_str() == "rokast" {
            let mut p: PathBuf = parts[..i].iter().collect();
            p.push("current");
            p.extend(&parts[i + 2..]);
            return p;
        }
    }
    exe.to_path_buf()
}

/// Pure: the autostart file for `exe`. Exec quoting per the Desktop Entry spec.
pub fn entry(exe: &Path) -> String {
    let mut q = String::new();
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            q.push('\\');
        }
        q.push(c);
    }
    format!(
        "[Desktop Entry]\nType=Application\nName=rokast\nComment=Mirror this screen to a Roku TV (starts in the tray)\n\
         Exec=\"{q}\" --tray\nIcon=io.github.papacasper.rokast\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

pub fn set(on: bool) -> std::io::Result<()> {
    let p = path();
    if !on {
        return match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let exe = exec_path(&std::env::current_exe()?);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, entry(&exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_copy_points_at_current() {
        let exe = Path::new("/home/u/.local/share/rokast/versions/0.2.0/ui/target/release/rokast-ui");
        assert_eq!(exec_path(exe), Path::new("/home/u/.local/share/rokast/current/ui/target/release/rokast-ui"));
    }

    #[test]
    fn git_checkout_is_left_alone() {
        let exe = Path::new("/home/u/dev/rokast/ui/target/release/rokast-ui");
        assert_eq!(exec_path(exe), exe);
    }

    #[test]
    fn entry_starts_in_tray_and_quotes_path() {
        let e = entry(Path::new("/home/my user/$x/rokast-ui"));
        assert!(e.contains("Exec=\"/home/my user/\\$x/rokast-ui\" --tray\n"), "{e}");
        assert!(e.starts_with("[Desktop Entry]\nType=Application\n"));
    }
}
