//! Which monitor to mirror (X11 only): a dropdown filled from `rokast monitors --json`, passed to the CLI as
//! MONITOR. Hidden unless there are two or more monitors (on Wayland the CLI lists none: the portal dialog picks).

use crate::config::rokast_cmd;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Deserialize, Clone)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

pub struct Picker {
    pub row: gtk::Box,
    pub drop: gtk::DropDown,
    names: Rc<RefCell<Vec<String>>>,
}

/// Pure: dropdown labels and MONITOR values; entry 0 is "follow the primary monitor" (MONITOR empty).
pub fn choices(mons: &[Monitor]) -> Vec<(String, String)> {
    let mut v = vec![("Main monitor".to_string(), String::new())];
    for m in mons {
        let main = if m.primary { ", main" } else { "" };
        v.push((format!("{} ({}x{}{main})", m.name, m.width, m.height), m.name.clone()));
    }
    v
}

impl Picker {
    pub fn new(row: impl Fn(&str, &gtk::DropDown) -> gtk::Box, preferred: String) -> Self {
        let model = gtk::StringList::new(&[]);
        let drop = gtk::DropDown::new(Some(model.clone()), None::<gtk::Expression>);
        let row = row("Monitor", &drop);
        row.set_visible(false);
        let names = Rc::new(RefCell::new(Vec::new()));
        let (tx, rx) = async_channel::bounded::<Vec<Monitor>>(1);
        std::thread::spawn(move || {
            let mons = rokast_cmd(&["monitors", "--json"])
                .output()
                .ok()
                .and_then(|o| serde_json::from_slice::<Vec<Monitor>>(&o.stdout).ok())
                .unwrap_or_default();
            let _ = tx.send_blocking(mons);
        });
        let (r, d, n) = (row.clone(), drop.clone(), names.clone());
        glib::spawn_future_local(async move {
            let Ok(mons) = rx.recv().await else { return };
            let c = choices(&mons);
            model.splice(0, 0, &c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>());
            *n.borrow_mut() = c.into_iter().map(|x| x.1).collect();
            d.set_selected(n.borrow().iter().position(|x| *x == preferred).unwrap_or(0) as u32);
            r.set_visible(mons.len() > 1);
        });
        Picker { row, drop, names }
    }

    /// MONITOR for the CLI ("" = primary; also before the list has loaded).
    pub fn selected(&self) -> String {
        self.names.borrow().get(self.drop.selected() as usize).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_choice_follows_primary() {
        let m = |n: &str, p| Monitor { name: n.into(), width: 1920, height: 1080, primary: p };
        let c = choices(&[m("eDP-1", true), m("HDMI-1", false)]);
        assert_eq!(c[0], ("Main monitor".into(), "".into()));
        assert_eq!(c[1], ("eDP-1 (1920x1080, main)".into(), "eDP-1".into()));
        assert_eq!(c[2].1, "HDMI-1");
    }
}
