//! "Fill the TV": FIT=match makes the laptop take the TV's size while mirroring, so the picture fills it (no black
//! bars when the laptop is 16:10 and the TV 16:9). X11 only; the CLI says so on Wayland and keeps the bars.
use gtk::prelude::*;

pub const TOOLTIP: &str = "While mirroring, the laptop screen switches to the TV's size so the picture fills the TV \
                           with no black bars. It switches back when you stop.";

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

#[cfg(test)]
mod tests {
    #[test]
    fn values() {
        assert_eq!(super::value(true), "match");
        assert_eq!(super::value(false), "bars");
    }
}
