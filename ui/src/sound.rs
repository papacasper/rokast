//! Where the sound goes: a dropdown passed to the CLI as AUDIO. Fixed choices first, then one entry per app playing
//! sound and per output, from `rokast audio --json` (reloaded with every TV search).

use crate::config::rokast_cmd;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Deserialize, Default, Clone)]
pub struct Listing {
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub apps: Vec<App>,
}

#[derive(Deserialize, Clone)]
pub struct Output {
    pub name: String,
    pub description: String,
}

#[derive(Deserialize, Clone)]
pub struct App {
    pub app: String,
}

const FIXED: [(&str, &str); 3] = [
    ("On the TV only", "tv"),
    ("On the TV and this computer", "both"),
    ("Not sent", "off"),
];

/// AUDIO from the saved prefs: `sound` when set, else the pre-0.6 on/off switch.
pub fn from_prefs(sound: &str, audio: bool) -> String {
    match sound {
        "" if audio => "tv".into(),
        "" => "off".into(),
        s => s.into(),
    }
}

/// Pure: dropdown labels and AUDIO values. A saved app/output that is not in the listing (the app is not playing
/// right now) keeps its entry so the choice survives.
pub fn choices(l: &Listing, saved: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = FIXED
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    for a in &l.apps {
        v.push((format!("Only {}", a.app), format!("app:{}", a.app)));
    }
    for o in &l.outputs {
        v.push((
            format!("Output: {}", o.description),
            format!("device:{}", o.name),
        ));
    }
    if !v.iter().any(|(_, x)| x == saved) {
        if let Some(app) = saved.strip_prefix("app:") {
            v.push((format!("Only {app}"), saved.into()));
        } else if let Some(dev) = saved.strip_prefix("device:") {
            v.push((format!("Output: {dev}"), saved.into()));
        }
    }
    v
}

pub struct Picker {
    pub drop: gtk::DropDown,
    model: gtk::StringList,
    values: Rc<RefCell<Vec<String>>>,
}

impl Picker {
    pub fn new(saved: String) -> Self {
        let c = choices(&Listing::default(), &saved);
        let model = gtk::StringList::new(&c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>());
        let drop = gtk::DropDown::new(Some(model.clone()), None::<gtk::Expression>);
        drop.set_tooltip_text(Some("Where the computer's sound plays while mirroring"));
        let values = Rc::new(RefCell::new(c.into_iter().map(|x| x.1).collect::<Vec<_>>()));
        drop.set_selected(
            values
                .borrow()
                .iter()
                .position(|x| *x == saved)
                .unwrap_or(0) as u32,
        );
        Picker {
            drop,
            model,
            values,
        } // filled by reload(), which every TV search calls
    }

    /// Ask the CLI which apps and outputs there are now, keeping the current choice.
    pub fn reload(&self) {
        let (tx, rx) = async_channel::bounded::<Listing>(1);
        std::thread::spawn(move || {
            let l = rokast_cmd(&["audio", "--json"])
                .output()
                .ok()
                .and_then(|o| serde_json::from_slice::<Listing>(&o.stdout).ok())
                .unwrap_or_default();
            let _ = tx.send_blocking(l);
        });
        let (d, m, v) = (self.drop.clone(), self.model.clone(), self.values.clone());
        glib::spawn_future_local(async move {
            let Ok(l) = rx.recv().await else { return };
            let current = v
                .borrow()
                .get(d.selected() as usize)
                .cloned()
                .unwrap_or_else(|| "tv".into());
            let c = choices(&l, &current);
            m.splice(
                0,
                m.n_items(),
                &c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            );
            *v.borrow_mut() = c.into_iter().map(|x| x.1).collect();
            d.set_selected(v.borrow().iter().position(|x| *x == current).unwrap_or(0) as u32);
        });
    }

    /// AUDIO for the CLI.
    pub fn selected(&self) -> String {
        self.values
            .borrow()
            .get(self.drop.selected() as usize)
            .cloned()
            .unwrap_or_else(|| "tv".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> Listing {
        serde_json::from_str(
            r#"{"outputs": [{"name": "alsa_output.hdmi", "description": "HDMI"}],
                "apps": [{"app": "Firefox", "media": "video"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn menu_from_cli_json() {
        let c = choices(&listing(), "tv");
        let values: Vec<&str> = c.iter().map(|x| x.1.as_str()).collect();
        assert_eq!(
            values,
            [
                "tv",
                "both",
                "off",
                "app:Firefox",
                "device:alsa_output.hdmi"
            ]
        );
        assert_eq!(c[3].0, "Only Firefox");
        assert_eq!(c[4].0, "Output: HDMI");
    }

    #[test]
    fn saved_choice_not_playing_is_kept() {
        let c = choices(&listing(), "app:mpv");
        assert_eq!(
            c.last().unwrap(),
            &("Only mpv".to_string(), "app:mpv".to_string())
        );
        assert_eq!(choices(&listing(), "app:Firefox").len(), 5); // already listed: no duplicate
        assert_eq!(
            choices(&Listing::default(), "device:x").last().unwrap().1,
            "device:x"
        );
    }

    #[test]
    fn old_switch_maps_to_tv_or_off() {
        assert_eq!(from_prefs("", true), "tv");
        assert_eq!(from_prefs("", false), "off");
        assert_eq!(from_prefs("both", false), "both");
    }
}
