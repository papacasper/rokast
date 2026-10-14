//! "Sound timing" (AUDIO_DELAY, milliseconds as in mpv): left plays the TV's sound earlier, for a TV that shows the
//! picture before the sound; right plays it later. Until the slider is moved, rokast.conf's AUDIO_DELAY applies.
use crate::config::{conf_value, Prefs};
use gtk::prelude::*;

pub const TOOLTIP: &str = "If the TV shows the picture before you hear the sound, slide left (sound earlier); if \
                           the sound comes first, slide right. Takes effect when you start mirroring.";
const MAX: f64 = 500.0;
const STEP: f64 = 50.0;

/// The slider's text: "−200 ms (earlier)", "0 ms", "150 ms (later)".
pub fn label(ms: i32) -> String {
    match ms {
        0 => "0 ms".into(),
        m if m < 0 => format!("−{} ms (earlier)", -m),
        m => format!("{m} ms (later)"),
    }
}

/// The starting value: the remembered one, else rokast.conf's, else 0 (kept within the slider).
pub fn initial(saved: Option<i32>, conf: &str) -> i32 {
    saved.or_else(|| conf.trim().parse().ok()).unwrap_or(0).clamp(-MAX as i32, MAX as i32)
}

pub struct Delay {
    pub widget: gtk::Box, // the slider and its value
    pub scale: gtk::Scale,
    moved: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Delay {
    pub fn new(prefs: &Prefs) -> Self {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -MAX, MAX, STEP);
        scale.set_value(f64::from(initial(prefs.audio_delay, &conf_value("AUDIO_DELAY"))));
        scale.set_draw_value(false);
        scale.set_width_request(160);
        scale.add_mark(0.0, gtk::PositionType::Bottom, None);
        scale.set_tooltip_text(Some(TOOLTIP));
        let text = gtk::Label::new(Some(&label(scale.value().round() as i32)));
        text.set_width_chars(15);
        text.set_xalign(1.0);
        text.add_css_class("dim-label");
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        widget.append(&text);
        widget.append(&scale);
        let moved = std::rc::Rc::new(std::cell::Cell::new(prefs.audio_delay.is_some()));
        let m = moved.clone();
        scale.connect_value_changed(move |s| {
            let snapped = (s.value() / STEP).round() * STEP; // keyboard and drags land on 50 ms steps
            if snapped != s.value() {
                s.set_value(snapped);
            }
            text.set_text(&label(s.value().round() as i32));
            m.set(true);
        });
        Delay { widget, scale, moved }
    }

    pub fn save(&self, p: &mut Prefs) {
        p.audio_delay = self.moved.get().then(|| self.scale.value().round() as i32);
    }

    pub fn env(prefs: &Prefs, cmd: &mut std::process::Command) {
        if let Some(ms) = prefs.audio_delay {
            cmd.env("AUDIO_DELAY", ms.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(label(0), "0 ms");
        assert_eq!(label(-200), "−200 ms (earlier)");
        assert_eq!(label(150), "150 ms (later)");
    }

    #[test]
    fn starts_from_prefs_then_conf() {
        assert_eq!(initial(Some(-100), "-300"), -100);
        assert_eq!(initial(None, " -300 "), -300);
        assert_eq!(initial(None, ""), 0);
        assert_eq!(initial(None, "abc"), 0);
        assert_eq!(initial(None, "-2000"), -500); // the CLI allows more; the slider shows its end
    }
}
