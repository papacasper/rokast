//! "rokast X.Y.Z is available [Update]": asks `rokast version --check --json` once at start (the CLI rate-limits to
//! one request a day and honours CHECK_UPDATES=0), then shows a banner under the status and a line in the tray menu.
//! Update runs `rokast update --yes` and shows its `update: <state>` lines; it is disabled while a mirror runs (the
//! CLI's session lock). With AUTO_UPDATE=1 the CLI installs at start by itself (`rokast update --auto`).
//! Installs that can't update themselves (AUR, git clone) get the old "Get it" link.

use crate::config::rokast_cmd;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Stdio;

#[derive(Deserialize, Default, Clone)]
pub struct Check {
    #[serde(default)]
    pub newer: bool,
    #[serde(default)]
    pub latest: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub packaged: bool,
    /// user (scripts/install), deb, rpm, arch, git, other; missing from a v0.4 CLI
    #[serde(default)]
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Available,
    Updating(String),
    Done,
    Failed(String),
}

/// Pure: does `rokast update` install for this kind of install?
pub fn can_run(kind: &str) -> bool {
    matches!(kind, "user" | "deb" | "rpm")
}

/// Pure: `update: <state> [detail]` -> (state, detail).
pub fn parse_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("update: ")?;
    let (state, detail) = rest.split_once(' ').unwrap_or((rest, ""));
    Some((state.to_string(), detail.to_string()))
}

/// Pure: what the banner says while `rokast update` runs.
pub fn progress_text(state: &str, detail: &str) -> Option<String> {
    Some(match state {
        "checking" => "Checking for the latest release…".into(),
        "downloading" => format!("Downloading {detail}…"),
        "verifying" => "Checking the signature…".into(),
        "installing" => format!("Installing {detail}…"),
        _ => return None,
    })
}

/// Pure: banner markup, or None when there is nothing newer.
pub fn banner_markup(c: &Check, phase: &Phase) -> Option<String> {
    let v = glib::markup_escape_text(c.latest.as_deref().filter(|_| c.newer)?);
    let link = format!("<a href=\"{}\">Get it</a>", glib::markup_escape_text(&c.url));
    Some(match phase {
        Phase::Available if can_run(&c.kind) => format!("rokast {v} is available."),
        Phase::Available if c.packaged || c.kind == "arch" => {
            format!("rokast {v} is available: update with your package manager")
        }
        Phase::Available => format!("rokast {v} is available. {link}"),
        Phase::Updating(s) => glib::markup_escape_text(s).to_string(),
        Phase::Done => format!("Updated to rokast {v}. Restart rokast to use it."),
        Phase::Failed(why) => format!("Update failed: {}. {link}", glib::markup_escape_text(why)),
    })
}

/// The lock `rokast start` holds while mirroring (update.lock_file in the CLI).
fn lock_file() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
    });
    base.join("rokast/session.lock")
}

pub fn mirroring() -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::File::open(lock_file()) else { return false };
    // SAFETY: flock on a file descriptor we own; a shared lock fails only while `rokast start` holds its lock.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

enum Msg {
    Line(String, String),
    Exit(bool),
}

/// Run `rokast <args>` in a thread, forwarding its `update:` lines and whether it exited 0.
fn run(args: &'static [&'static str]) -> async_channel::Receiver<Msg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let child = rokast_cmd(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else {
            let _ = tx.send_blocking(Msg::Line("error".into(), "could not run rokast".into()));
            let _ = tx.send_blocking(Msg::Exit(false));
            return;
        };
        if let Some(out) = child.stdout.take() {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Some((s, d)) = parse_line(&line) {
                    let _ = tx.send_blocking(Msg::Line(s, d));
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send_blocking(Msg::Exit(ok));
    });
    rx
}

/// Start the new version (after this one has quit) and quit: `rokast ui`, with --tray if we were started that way.
fn relaunch(kind: &str) {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let script = if kind == "user" { data.join("rokast/current/scripts/rokast") } else { crate::config::rokast_script() };
    let tray = if std::env::args().any(|a| a == "--tray") { " --tray" } else { "" };
    let cmd = format!("sleep 1; exec \"$0\" ui{tray}");
    let _ = std::process::Command::new("sh").args(["-c", &cmd]).arg(script).stdin(Stdio::null()).spawn();
    std::process::exit(0);
}

struct Banner {
    check: Check,
    label: gtk::Label,
    update: gtk::Button,
    restart: gtk::Button,
    tray: Option<ksni::blocking::Handle<crate::tray::Tray>>,
}

impl Banner {
    fn show(&self, phase: &Phase) {
        let Some(markup) = banner_markup(&self.check, phase) else { return };
        self.label.set_markup(&markup);
        let runs = can_run(&self.check.kind);
        self.update.set_visible(runs && matches!(phase, Phase::Available | Phase::Failed(_)));
        self.restart.set_visible(*phase == Phase::Done);
        if let Some(h) = &self.tray {
            let v = self.check.latest.clone().unwrap_or_default();
            let (label, url) = match phase {
                Phase::Done => (format!("Updated to {v}: restart rokast"), String::new()),
                Phase::Updating(s) => (s.clone(), String::new()),
                _ if runs => (format!("Update to {v}"), String::new()),
                _ if self.check.packaged || self.check.kind == "arch" => (format!("rokast {v} is available"), String::new()),
                _ => (format!("rokast {v} is available"), self.check.url.clone()),
            };
            let can = runs && matches!(phase, Phase::Available | Phase::Failed(_));
            h.update(move |t| {
                t.update = Some((label, url));
                t.update_can_run = can;
            });
        }
    }

    /// Run `rokast update <args>`; `auto` = the start-up AUTO_UPDATE run, which stays quiet when it skips.
    async fn go(self: std::rc::Rc<Self>, args: &'static [&'static str], auto: bool) {
        if !auto {
            self.show(&Phase::Updating("Starting the update…".into()));
        }
        let rx = run(args);
        let (mut last_error, mut skipped) = (String::new(), false);
        while let Ok(m) = rx.recv().await {
            match m {
                Msg::Line(s, d) => match s.as_str() {
                    "error" => last_error = d,
                    "skipped" => skipped = true,
                    _ => {
                        if let Some(t) = progress_text(&s, &d) {
                            self.show(&Phase::Updating(t));
                        }
                    }
                },
                Msg::Exit(ok) => {
                    let phase = if auto && skipped {
                        Phase::Available
                    } else if ok {
                        Phase::Done
                    } else {
                        Phase::Failed(if last_error.is_empty() { "see `rokast update`".into() } else { last_error.clone() })
                    };
                    self.show(&phase);
                }
            }
        }
    }
}

pub fn spawn(root: &gtk::Box, after: &gtk::Box, tray: Option<ksni::blocking::Handle<crate::tray::Tray>>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_hexpand(true);
    label.add_css_class("update-banner");
    let update = gtk::Button::with_label("Update");
    update.add_css_class("suggested-action");
    let restart = gtk::Button::with_label("Restart rokast");
    restart.add_css_class("suggested-action");
    row.append(&label);
    row.append(&update);
    row.append(&restart);
    row.set_visible(false);
    root.insert_child_after(&row, Some(after));
    let (tx, rx) = async_channel::bounded::<Check>(1);
    std::thread::spawn(move || {
        let c = rokast_cmd(&["version", "--check", "--json"])
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice::<Check>(&o.stdout).ok())
            .unwrap_or_default();
        let _ = tx.send_blocking(c);
    });
    glib::spawn_future_local(async move {
        let Ok(c) = rx.recv().await else { return };
        if banner_markup(&c, &Phase::Available).is_none() {
            return;
        }
        let kind = c.kind.clone();
        let (run_tx, run_rx) = async_channel::unbounded::<()>();
        if let Some(h) = &tray {
            h.update(move |t| t.update_tx = Some(run_tx));
        }
        let b = std::rc::Rc::new(Banner { check: c, label, update, restart, tray });
        b.show(&Phase::Available);
        row.set_visible(true);
        let bb = b.clone();
        b.update.connect_clicked(move |_| {
            glib::spawn_future_local(bb.clone().go(&["update", "--yes"], false));
        });
        b.restart.connect_clicked(move |_| relaunch(&kind));
        let bb = b.clone();
        glib::spawn_future_local(async move {
            while run_rx.recv().await.is_ok() {
                if bb.update.is_visible() && bb.update.is_sensitive() {
                    bb.clone().go(&["update", "--yes"], false).await;
                }
            }
        });
        // Not while mirroring: the CLI would refuse anyway; say so before anyone clicks.
        let bb = b.clone();
        let follow = move || {
            let busy = mirroring();
            bb.update.set_sensitive(!busy);
            bb.update.set_tooltip_text(busy.then_some("Stop mirroring first"));
            glib::ControlFlow::Continue
        };
        follow();
        glib::timeout_add_seconds_local(2, follow);
        if b.check.kind == "user" {
            b.go(&["update", "--auto"], true).await;   // installs only with AUTO_UPDATE=1 (the CLI decides)
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(newer: bool, kind: &str, packaged: bool) -> Check {
        Check { newer, latest: Some("9.9.9".into()), url: "https://x/r?a&b".into(), packaged, kind: kind.into() }
    }

    #[test]
    fn banner_per_kind_and_phase() {
        assert_eq!(banner_markup(&c(false, "user", false), &Phase::Available), None);
        assert_eq!(banner_markup(&Check::default(), &Phase::Available), None);
        assert_eq!(banner_markup(&c(true, "user", false), &Phase::Available).unwrap(), "rokast 9.9.9 is available.");
        assert_eq!(banner_markup(&c(true, "deb", true), &Phase::Available).unwrap(), "rokast 9.9.9 is available.");
        assert!(banner_markup(&c(true, "arch", true), &Phase::Available).unwrap().ends_with("your package manager"));
        assert_eq!(
            banner_markup(&c(true, "git", false), &Phase::Available).unwrap(),
            "rokast 9.9.9 is available. <a href=\"https://x/r?a&amp;b\">Get it</a>"
        );
        // a v0.4 CLI sends no kind: packaged -> package manager, else the link
        assert!(banner_markup(&c(true, "", true), &Phase::Available).unwrap().ends_with("your package manager"));
        assert!(banner_markup(&c(true, "", false), &Phase::Available).unwrap().contains("Get it"));
        let u = c(true, "user", false);
        assert_eq!(banner_markup(&u, &Phase::Updating("Checking the signature…".into())).unwrap(), "Checking the signature…");
        assert_eq!(banner_markup(&u, &Phase::Done).unwrap(), "Updated to rokast 9.9.9. Restart rokast to use it.");
        assert_eq!(
            banner_markup(&u, &Phase::Failed("stop mirroring <first>".into())).unwrap(),
            "Update failed: stop mirroring &lt;first&gt;. <a href=\"https://x/r?a&amp;b\">Get it</a>"
        );
    }

    #[test]
    fn mirroring_follows_the_session_lock() {
        use std::os::fd::AsRawFd;
        let d = std::env::temp_dir().join(format!("rokast-ui-lock-{}", std::process::id()));
        std::fs::create_dir_all(d.join("rokast")).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &d); // the only test that reads it
        assert!(!mirroring());
        let f = std::fs::File::create(d.join("rokast/session.lock")).unwrap();
        assert!(!mirroring());
        // SAFETY: flock on a descriptor we own
        assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        assert!(mirroring());
        drop(f);
        assert!(!mirroring());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn update_lines() {
        assert_eq!(parse_line("update: downloading SHA256SUMS"), Some(("downloading".into(), "SHA256SUMS".into())));
        assert_eq!(parse_line("update: verifying"), Some(("verifying".into(), String::new())));
        assert_eq!(parse_line("status: streaming"), None);
        assert_eq!(progress_text("installing", "0.5.0").unwrap(), "Installing 0.5.0…");
        assert_eq!(progress_text("log", "x"), None);
        assert!(can_run("user") && can_run("deb") && can_run("rpm"));
        assert!(!can_run("arch") && !can_run("git") && !can_run("other") && !can_run(""));
    }
}
