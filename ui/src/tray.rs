//! System tray icon (StatusNotifierItem: XFCE, KDE, Cinnamon, MATE, LXQt, sway/Hyprland bars;
//! GNOME needs the AppIndicator extension). The icon is drawn in code so it looks the same in every
//! icon theme, and its colour shows the state at a glance.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

use super::{stop, State, Ui};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Look {
    Idle,
    Busy,
    Mirroring,
    Error,
}

/// What the tray asks the window to do; delivered on the GTK main thread.
#[derive(Debug)]
pub enum Cmd {
    ToggleWindow,
    StartStop,
    Quit,
    Gone,
}

pub struct Tray {
    pub look: Look,
    pub status: String,
    pub detail: String,
    pub running: bool,
    pub can_start: bool,
    pub window_visible: bool,
    /// (menu label, release page; empty for packaged installs) once a newer release is known
    pub update: Option<(String, String)>,
    /// the tray line runs `rokast update --yes` (sent to the banner) instead of opening the page
    pub update_can_run: bool,
    pub update_tx: Option<async_channel::Sender<()>>,
    pub tx: async_channel::Sender<Cmd>,
}

impl Tray {
    fn send(&self, c: Cmd) {
        let _ = self.tx.send_blocking(c);
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "rokast".into()
    }
    fn title(&self) -> String {
        "rokast".into()
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }
    fn status(&self) -> ksni::Status {
        if self.look == Look::Error { ksni::Status::NeedsAttention } else { ksni::Status::Active }
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [16, 22, 24, 32, 48, 64].iter().map(|&s| tv_icon(s, self.look)).collect()
    }
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon_pixmap()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: format!("rokast: {}", self.status), description: self.detail.clone(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::ToggleWindow);
    }
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.send(Cmd::Gone); // panel/tray host went away: the window must not stay hidden with no way back
        true
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();
        if let Some((label, url)) = self.update.clone() {
            items.push(
                StandardItem {
                    label,
                    icon_name: "software-update-available".into(),
                    enabled: self.update_can_run || !url.is_empty(),
                    activate: Box::new(move |t: &mut Self| {
                        if t.update_can_run {
                            if let Some(tx) = &t.update_tx {
                                let _ = tx.send_blocking(());
                            }
                        } else if !url.is_empty() {
                            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.extend([
            StandardItem { label: self.status.replace('_', "__"), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: if self.running { "Stop mirroring".into() } else { "Start mirroring".into() },
                icon_name: if self.running { "media-playback-stop".into() } else { "media-playback-start".into() },
                enabled: self.running || self.can_start,
                activate: Box::new(|t: &mut Self| t.send(Cmd::StartStop)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.window_visible { "Hide window".into() } else { "Show window".into() },
                activate: Box::new(|t: &mut Self| t.send(Cmd::ToggleWindow)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Quit)),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

/// Start the tray service; `None` when the desktop has no StatusNotifier host.
pub fn spawn() -> (Option<ksni::blocking::Handle<Tray>>, async_channel::Receiver<Cmd>) {
    let (tx, rx) = async_channel::unbounded();
    let tray = Tray {
        look: Look::Idle, status: "Ready".into(), detail: String::new(), running: false, can_start: false,
        window_visible: true, update: None, update_can_run: false, update_tx: None, tx,
    };
    (tray.spawn().ok(), rx)
}

pub fn command(ui: &Ui, st: &Rc<RefCell<State>>, cmd: Cmd) {
    match cmd {
        Cmd::ToggleWindow if ui.window.is_visible() => ui.window.set_visible(false),
        Cmd::ToggleWindow | Cmd::Gone => ui.window.present(),
        Cmd::StartStop => {
            if ui.button.is_sensitive() {
                ui.button.emit_clicked();
            }
        }
        Cmd::Quit => {
            st.borrow_mut().quit_after = true;
            if st.borrow().child.is_some() {
                stop(ui, st); // the Exit event destroys the window once the link is torn down
            } else {
                ui.window.destroy();
            }
        }
    }
}

pub fn sync(ui: &Ui) {
    let Some(handle) = &ui.tray else { return };
    let title = ui.title.text().to_string();
    let look = if title == "Mirroring" {
        Look::Mirroring
    } else if title.starts_with("Couldn") || title.starts_with("Can't") {
        Look::Error
    } else if ui.spinner.is_visible() {
        Look::Busy
    } else {
        Look::Idle
    };
    let tv = ui.tv_model.string(ui.tv_drop.selected()).map(|s| s.split("  (").next().unwrap_or("").to_string());
    let status = match (look, tv) {
        (Look::Mirroring, Some(tv)) => format!("Mirroring to {tv}"),
        _ => title,
    };
    let (detail, running) = (ui.detail.text().to_string(), ui.button.has_css_class("destructive-action"));
    let (can_start, visible) = (ui.button.is_sensitive() && !running, ui.window.is_visible());
    handle.update(move |t| {
        t.look = look;
        t.status = status;
        t.detail = detail;
        t.running = running;
        t.can_start = can_start;
        t.window_visible = visible;
    });
}

/// A small TV: light bezel, screen coloured by state, stand underneath. ARGB32, network byte order.
pub fn tv_icon(size: i32, look: Look) -> ksni::Icon {
    let screen: [u8; 3] = match look {
        Look::Idle => [0x5e, 0x5c, 0x64],
        Look::Busy => [0xf5, 0xa6, 0x23],
        Look::Mirroring => [0x2e, 0xc2, 0x7e],
        Look::Error => [0xe0, 0x1b, 0x24],
    };
    let bezel = [0xde, 0xdd, 0xda];
    let s = size as f32;
    let b = (s / 12.0).max(1.5);
    let (x0, x1, y0, y1) = (s * 0.06, s * 0.94, s * 0.12, s * 0.74);
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let in_tv = fx >= x0 && fx <= x1 && fy >= y0 && fy <= y1;
            let in_screen = fx >= x0 + b && fx <= x1 - b && fy >= y0 + b && fy <= y1 - b;
            let in_neck = (fx - s / 2.0).abs() <= (s * 0.06).max(1.0) && fy > y1 && fy <= s * 0.84;
            let in_foot = fx >= s * 0.28 && fx <= s * 0.72 && fy > s * 0.84 && fy <= s * 0.84 + b.max(2.0);
            let px = if in_screen {
                Some(screen)
            } else if in_tv || in_neck || in_foot {
                Some(bezel)
            } else {
                None
            };
            match px {
                Some([r, g, bl]) => data.extend_from_slice(&[0xff, r, g, bl]),
                None => data.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
    }
    ksni::Icon { width: size, height: size, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_argb_and_coloured_by_state() {
        let i = tv_icon(32, Look::Mirroring);
        assert_eq!(i.data.len(), 32 * 32 * 4);
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(&i.data[centre..centre + 4], &[0xff, 0x2e, 0xc2, 0x7e]);
        assert_eq!(&i.data[0..4], &[0, 0, 0, 0]); // transparent corner
        assert_ne!(tv_icon(32, Look::Idle).data, i.data);
    }
}
