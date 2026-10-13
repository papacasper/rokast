//! The picture settings: "Fill the TV" (FIT=match: the laptop takes the TV's size while mirroring, so a 16:10 laptop
//! fills a 16:9 TV) and "TV as a second screen" (SCREEN=extend: the TV is an extra screen beside the laptop instead
//! of a copy; EXTEND_SIDE in rokast.conf picks the side). X11 only; the CLI says so on Wayland and mirrors.
//! "Smooth motion" (FPS=60): 60 frames a second instead of the 30 the TV offers, in every mode.
use crate::config::Prefs;
use gtk::prelude::*;

pub const TOOLTIP: &str = "While mirroring, the laptop screen switches to the TV's size so the picture fills the TV \
                           with no black bars. It switches back when you stop.";
pub const EXTEND_TOOLTIP: &str = "The TV becomes a second screen to the right of the laptop (EXTEND_SIDE in \
                                  rokast.conf for left, above or below): drag windows onto it. Gone when you stop.";
pub const SMOOTH_TOOLTIP: &str = "Sends 60 frames a second instead of the 30 the TV asks for, so movement is \
                                  smoother. The laptop screen runs at 60 Hz while mirroring.";

/// A switch for a form row.
pub fn switch(active: bool, tooltip: &str) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_valign(gtk::Align::Center);
    s.set_active(active);
    s.set_tooltip_text(Some(tooltip));
    s
}

/// The FIT value for the switch's state.
pub fn value(fill: bool) -> &'static str {
    if fill { "match" } else { "bars" }
}

/// The FPS value for the switch's state.
pub fn fps(smooth: bool) -> &'static str {
    if smooth { "60" } else { "tv" }
}

/// The SCREEN value for the switch's state.
pub fn screen(extend: bool) -> &'static str {
    if extend { "extend" } else { "mirror" }
}

pub struct Screen {
    pub fill: gtk::Switch,
    pub extend: gtk::Switch,
    pub smooth: gtk::Switch,
}

impl Screen {
    pub fn new(prefs: &Prefs) -> Self {
        let s = Screen {
            fill: switch(prefs.fill, TOOLTIP),
            extend: switch(prefs.extend, EXTEND_TOOLTIP),
            smooth: switch(prefs.smooth, SMOOTH_TOOLTIP),
        };
        s.fill.set_sensitive(!prefs.extend); // a second screen is the TV's size already
        let fill = s.fill.clone();
        s.extend.connect_active_notify(move |e| fill.set_sensitive(e.is_sensitive() && !e.is_active()));
        s
    }

    pub fn rows(&self, form: &gtk::Box, row: impl Fn(&str, &gtk::Switch) -> gtk::Box) {
        form.append(&row("Fill the TV (laptop takes its size)", &self.fill));
        form.append(&row("TV as a second screen (extend)", &self.extend));
        form.append(&row("Smooth motion (60 fps)", &self.smooth));
    }

    pub fn set_sensitive(&self, on: bool) {
        self.extend.set_sensitive(on);
        self.smooth.set_sensitive(on);
        self.fill.set_sensitive(on && !self.extend.is_active());
    }

    pub fn save(&self, p: &mut Prefs) {
        (p.fill, p.extend, p.smooth) = (self.fill.is_active(), self.extend.is_active(), self.smooth.is_active());
    }

    pub fn env(prefs: &Prefs, cmd: &mut std::process::Command) {
        cmd.env("FIT", value(prefs.fill)).env("SCREEN", screen(prefs.extend)).env("FPS", fps(prefs.smooth));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn values() {
        assert_eq!(super::value(true), "match");
        assert_eq!(super::value(false), "bars");
        assert_eq!(super::screen(true), "extend");
        assert_eq!(super::screen(false), "mirror");
        assert_eq!(super::fps(true), "60");
        assert_eq!(super::fps(false), "tv");
    }
}
