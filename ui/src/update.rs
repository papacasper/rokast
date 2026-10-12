//! "rokast X.Y.Z is available": asks `rokast version --check --json` once at start (the CLI rate-limits to one
//! request a day and honours CHECK_UPDATES=0), then shows a banner under the status and a line in the tray menu.

use crate::config::rokast_cmd;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct Check {
    #[serde(default)]
    pub newer: bool,
    #[serde(default)]
    pub latest: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub packaged: bool,
}

/// Pure: banner markup, or None when there is nothing newer.
pub fn banner_markup(c: &Check) -> Option<String> {
    let v = c.latest.as_deref().filter(|_| c.newer)?;
    let v = glib::markup_escape_text(v);
    Some(if c.packaged {
        format!("rokast {v} is available: update with your package manager")
    } else {
        format!("rokast {v} is available. <a href=\"{}\">Get it</a>", glib::markup_escape_text(&c.url))
    })
}

pub fn spawn(root: &gtk::Box, after: &gtk::Box, tray: Option<ksni::blocking::Handle<crate::tray::Tray>>) {
    let banner = gtk::Label::new(None);
    banner.set_xalign(0.0);
    banner.set_wrap(true);
    banner.add_css_class("update-banner");
    banner.set_visible(false);
    root.insert_child_after(&banner, Some(after));
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
        let Some(markup) = banner_markup(&c) else { return };
        banner.set_markup(&markup);
        banner.set_visible(true);
        if let (Some(h), Some(v)) = (tray, c.latest) {
            let url = if c.packaged { String::new() } else { c.url };
            h.update(move |t| t.update = Some((format!("rokast {v} is available"), url)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_only_when_newer() {
        let c = |newer, packaged| Check { newer, latest: Some("9.9.9".into()), url: "https://x/r?a&b".into(), packaged };
        assert_eq!(banner_markup(&c(false, false)), None);
        assert_eq!(banner_markup(&c(true, false)).unwrap(), "rokast 9.9.9 is available. <a href=\"https://x/r?a&amp;b\">Get it</a>");
        assert!(banner_markup(&c(true, true)).unwrap().ends_with("update with your package manager"));
        assert_eq!(banner_markup(&Check::default()), None);
    }
}
