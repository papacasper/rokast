//! rokast desktop UI (GTK4). A thin front end over `scripts/rokast`: it lists TVs with
//! `rokast list --json`, runs `rokast start <tv>` as a child process and follows its
//! `status: <state> [detail]` lines. All networking stays in the CLI, so both behave identically.
#![allow(deprecated)] // CssProvider::load_from_data / style_context_add_provider_for_display (kept for GTK 4.6)

mod autostart;
mod config;
mod monitors;
mod tray;
mod update;

use config::{conf_target, load_prefs, rokast_cmd, rokast_script, save_prefs, update_prefs, Prefs};

use gtk::prelude::*;
use gtk::{gio, glib};
use serde::Deserialize;
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::rc::Rc;

const APP_ID: &str = "io.github.papacasper.rokast";
const RES: [(&str, Option<&str>); 4] = [
    ("Best available", None),
    ("1080p 30 fps", Some("1080p30,720p60,720p30")),
    ("720p 60 fps", Some("720p60,720p30")),
    ("720p 30 fps", Some("720p30")),
];
const BITRATES: [&str; 4] = ["4M", "8M", "12M", "20M"];
const CSS: &str = "
.status-title { font-size: 20pt; font-weight: 800; }
.status-detail { opacity: 0.75; }
.big-button { font-size: 14pt; font-weight: 700; padding: 10px 0; }
.log { font-family: monospace; font-size: 9pt; }
.update-banner { font-weight: 600; }
";

#[derive(Deserialize, Clone)]
struct Tv {
    name: String,
    ip: String,
    model: String,
}

enum Ev {
    Tvs(Result<Vec<Tv>, String>),
    DefaultName(String),
    Line(String),
    Exit(i32),
}

struct Ui {
    window: gtk::ApplicationWindow,
    tv_model: gtk::StringList,
    tv_drop: gtk::DropDown,
    monitor: monitors::Picker,
    refresh: gtk::Button,
    res_drop: gtk::DropDown,
    rate_drop: gtk::DropDown,
    audio: gtk::Switch,
    name: gtk::Entry,
    title: gtk::Label,
    detail: gtk::Label,
    spinner: gtk::Spinner,
    button: gtk::Button,
    log: gtk::TextView,
    tray: Option<ksni::blocking::Handle<tray::Tray>>,
}

#[derive(Default)]
struct State {
    tvs: Vec<Tv>,
    child: Option<i32>,
    stopping: bool,
    quit_after: bool,
    last_error: Option<String>,
}

fn row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    b.append(&l);
    b.append(widget);
    b
}

fn build(app: &gtk::Application) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = gtk::ApplicationWindow::builder().application(app).title("rokast").default_width(440).build();
    let header = gtk::HeaderBar::new();
    window.set_titlebar(Some(&header));
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Search for TVs again"));
    header.pack_start(&refresh);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(18);
    root.set_margin_end(18);

    // Status
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(28, 28);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(Some("Ready"));
    title.add_css_class("status-title");
    title.set_xalign(0.0);
    let detail = gtk::Label::new(Some("Searching for TVs on your network…"));
    detail.add_css_class("status-detail");
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    texts.append(&title);
    texts.append(&detail);
    status.append(&spinner);
    status.append(&texts);
    root.append(&status);

    // Controls
    let tv_model = gtk::StringList::new(&[]);
    let tv_drop = gtk::DropDown::new(Some(tv_model.clone()), None::<gtk::Expression>);
    tv_drop.set_hexpand(true);
    let res_drop = gtk::DropDown::from_strings(&RES.map(|r| r.0));
    let rate_drop = gtk::DropDown::from_strings(&BITRATES.map(|b| b.trim_end_matches('M')).map(|b| match b {
        "4" => "4 Mbit/s (weak Wi-Fi)",
        "8" => "8 Mbit/s",
        "12" => "12 Mbit/s",
        _ => "20 Mbit/s (sharpest)",
    }));
    let audio = gtk::Switch::new();
    audio.set_valign(gtk::Align::Center);
    let name = gtk::Entry::new();
    name.set_max_length(32); // Wi-Fi Direct device-name limit (bytes; the CLI trims multi-byte names further)
    name.set_width_chars(18);
    name.set_tooltip_text(Some("What the TV calls this computer in its screen-mirroring prompt and device list"));

    let prefs = load_prefs();
    let monitor = monitors::Picker::new(|l, w| row(l, w), prefs.monitor.clone());
    let boot = gtk::Switch::new();
    boot.set_valign(gtk::Align::Center);
    boot.set_active(autostart::enabled());
    boot.set_tooltip_text(Some("Open rokast as a tray icon (no window) when you log in"));

    let frame = gtk::Frame::new(None);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 10);
    form.set_margin_top(12);
    form.set_margin_bottom(12);
    form.set_margin_start(12);
    form.set_margin_end(12);
    form.append(&row("TV", &tv_drop));
    form.append(&monitor.row);
    form.append(&row("Name on TV", &name));
    form.append(&row("Quality", &res_drop));
    form.append(&row("Bitrate", &rate_drop));
    form.append(&row("Send audio", &audio));
    form.append(&row("Start with my computer, in the tray", &boot));
    frame.set_child(Some(&form));
    root.append(&frame);

    let button = gtk::Button::with_label("Start mirroring");
    button.add_css_class("big-button");
    button.add_css_class("suggested-action");
    button.set_sensitive(false);
    root.append(&button);

    // Log
    let log = gtk::TextView::new();
    log.set_editable(false);
    log.set_cursor_visible(false);
    log.set_wrap_mode(gtk::WrapMode::WordChar);
    log.add_css_class("log");
    let scroll = gtk::ScrolledWindow::builder().child(&log).min_content_height(160).vexpand(true).build();
    let expander = gtk::Expander::new(Some("Details"));
    expander.set_child(Some(&scroll));
    root.append(&expander);
    window.set_child(Some(&root));

    if prefs.width > 0 && prefs.height > 0 {
        window.set_default_size(prefs.width, prefs.height);
    }
    expander.set_expanded(prefs.details);
    expander.connect_expanded_notify(|e| update_prefs(|p| p.details = e.is_expanded()));
    res_drop.set_selected(prefs.res.min(RES.len() as u32 - 1));
    rate_drop.set_selected(prefs.bitrate.min(BITRATES.len() as u32 - 1));
    audio.set_active(prefs.audio);
    name.set_text(&prefs.name);

    // Tray icon; None when the desktop has no StatusNotifier host (then closing the window quits, as before).
    let (tray, tray_rx) = tray::spawn();
    update::spawn(&root, &status, tray.clone());

    let ui = Rc::new(Ui {
        window, tv_model, tv_drop, monitor, refresh, res_drop, rate_drop, audio, name, title, detail, spinner, button, log, tray,
    });
    let st = Rc::new(RefCell::new(State::default()));
    let (tx, rx) = async_channel::unbounded::<Ev>();

    {
        let (ui, st) = (ui.clone(), st.clone());
        glib::spawn_future_local(async move {
            while let Ok(cmd) = tray_rx.recv().await {
                tray::command(&ui, &st, cmd);
            }
        });
    }
    // Keep the tray menu in step with the window without touching every call site.
    {
        let u = ui.clone();
        ui.button.connect_sensitive_notify(move |_| tray::sync(&u));
        let u = ui.clone();
        ui.window.connect_visible_notify(move |_| tray::sync(&u));
    }

    // Event loop: all child/scan output arrives here on the GTK main thread.
    {
        let (ui, st) = (ui.clone(), st.clone());
        let preferred = if prefs.target.is_empty() { conf_target() } else { prefs.target.clone() };
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                handle(&ui, &st, ev, &preferred);
            }
        });
    }

    {
        let (ui, st, tx) = (ui.clone(), st.clone(), tx.clone());
        ui.refresh.clone().connect_clicked(move |_| scan(&ui, &st, &tx));
    }
    {
        let (ui2, st, tx) = (ui.clone(), st.clone(), tx.clone());
        ui.button.connect_clicked(move |_| {
            if st.borrow().child.is_some() {
                stop(&ui2, &st);
            } else {
                start(&ui2, &st, &tx);
            }
        });
    }
    {
        let ui2 = ui.clone();
        boot.connect_state_set(move |sw, on| {
            if let Err(e) = autostart::set(on) {
                set_status(&ui2, "Couldn't change autostart", &format!("{}: {e}", autostart::path().display()), false);
                sw.set_active(autostart::enabled());
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    {
        let st = st.clone();
        let ui2 = ui.clone();
        ui.window.connect_close_request(move |w| {
            let (width, height) = w.default_size();
            update_prefs(|p| (p.width, p.height) = (width, height));
            // With a tray icon, closing only hides the window (mirroring carries on); Quit is in the tray menu.
            if ui2.tray.is_some() && !st.borrow().quit_after {
                w.set_visible(false);
                return glib::Propagation::Stop;
            }
            // Never leave the Wi-Fi Direct link half up: stop the child and close once it has cleaned up.
            if st.borrow().child.is_some() {
                st.borrow_mut().quit_after = true;
                stop(&ui2, &st);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }

    if prefs.name.is_empty() {
        // Show the name the CLI would use (config NAME, else this computer's name) instead of a blank box.
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Ok(o) = rokast_cmd(&["name"]).output() {
                let _ = tx.send_blocking(Ev::DefaultName(String::from_utf8_lossy(&o.stdout).trim().to_string()));
            }
        });
    }
    scan(&ui, &st, &tx);
    // `--tray`: start hidden in the tray (e.g. from autostart); only when there is a tray to come back from.
    if !(ui.tray.is_some() && std::env::args().any(|a| a == "--tray")) {
        ui.window.present();
    }
}

fn append_log(ui: &Ui, line: &str) {
    let buf = ui.log.buffer();
    buf.insert(&mut buf.end_iter(), &format!("{line}\n"));
    let mark = buf.create_mark(None, &buf.end_iter(), false);
    ui.log.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
    buf.delete_mark(&mark);
}

fn set_status(ui: &Ui, title: &str, detail: &str, busy: bool) {
    ui.title.set_text(title);
    ui.detail.set_text(detail);
    ui.spinner.set_spinning(busy);
    ui.spinner.set_visible(busy);
    tray::sync(ui);
}

fn set_running(ui: &Ui, running: bool) {
    for w in [ui.tv_drop.upcast_ref::<gtk::Widget>(), ui.monitor.drop.upcast_ref(), ui.res_drop.upcast_ref(),
              ui.rate_drop.upcast_ref(),
              ui.audio.upcast_ref(), ui.name.upcast_ref(), ui.refresh.upcast_ref()] {
        w.set_sensitive(!running);
    }
    ui.button.set_label(if running { "Stop mirroring" } else { "Start mirroring" });
    if running {
        ui.button.remove_css_class("suggested-action");
        ui.button.add_css_class("destructive-action");
    } else {
        ui.button.remove_css_class("destructive-action");
        ui.button.add_css_class("suggested-action");
    }
    tray::sync(ui);
}

fn scan(ui: &Ui, st: &Rc<RefCell<State>>, tx: &async_channel::Sender<Ev>) {
    if st.borrow().child.is_some() {
        return;
    }
    set_status(ui, "Searching", "Looking for Roku TVs on your network…", true);
    ui.refresh.set_sensitive(false);
    ui.button.set_sensitive(false);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let res = rokast_cmd(&["list", "--json"]).stderr(Stdio::piped()).output().map_err(|e| e.to_string()).and_then(|o| {
            if o.status.success() {
                serde_json::from_slice::<Vec<Tv>>(&o.stdout).map_err(|e| e.to_string())
            } else {
                Err(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("rokast list failed").to_string())
            }
        });
        let _ = tx.send_blocking(Ev::Tvs(res));
    });
}

fn start(ui: &Ui, st: &Rc<RefCell<State>>, tx: &async_channel::Sender<Ev>) {
    let Some(tv) = st.borrow().tvs.get(ui.tv_drop.selected() as usize).cloned() else { return };
    let prefs = Prefs {
        target: tv.name.clone(),
        res: ui.res_drop.selected(),
        bitrate: ui.rate_drop.selected(),
        audio: ui.audio.is_active(),
        name: ui.name.text().trim().to_string(),
        monitor: ui.monitor.selected(),
        ..load_prefs()
    };
    save_prefs(&prefs);

    let mut cmd = rokast_cmd(&["start", &tv.name]);
    cmd.env("BITRATE", BITRATES[prefs.bitrate as usize]).env("AUDIO", if prefs.audio { "1" } else { "0" });
    if let Some(res) = RES[prefs.res as usize].1 {
        cmd.env("RES", res);
    }
    if !prefs.name.is_empty() {
        cmd.env("NAME", &prefs.name);
    }
    if ui.monitor.row.is_visible() {
        cmd.env("MONITOR", &prefs.monitor); // "" = main monitor; hidden picker: rokast.conf decides
    }
    let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return set_status(ui, "Can't start", &format!("{}: {e}", rokast_script().display()), false),
    };
    ui.log.buffer().set_text("");
    {
        let mut s = st.borrow_mut();
        s.child = Some(child.id() as i32);
        s.stopping = false;
        s.last_error = None;
    }
    set_running(ui, true);
    set_status(ui, "Starting", &format!("Preparing to mirror to {}…", tv.name), true);

    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let tx = tx.clone();
    std::thread::spawn(move || {
        let pump = |r: Box<dyn std::io::Read + Send>, tx: async_channel::Sender<Ev>| {
            std::thread::spawn(move || {
                for line in BufReader::new(r).lines().map_while(Result::ok) {
                    let _ = tx.send_blocking(Ev::Line(line));
                }
            })
        };
        let (a, b) = (pump(Box::new(out), tx.clone()), pump(Box::new(err), tx.clone()));
        let _ = (a.join(), b.join());
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        let _ = tx.send_blocking(Ev::Exit(code));
    });
}

fn stop(ui: &Ui, st: &Rc<RefCell<State>>) {
    let mut s = st.borrow_mut();
    let Some(pid) = s.child else { return };
    if !s.stopping {
        s.stopping = true;
        // SIGTERM: the CLI tears down the stream, the patched supplicant and the P2P interface.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    ui.button.set_sensitive(false);
    set_status(ui, "Stopping", "Disconnecting from the TV and cleaning up…", true);
}

fn handle(ui: &Ui, st: &Rc<RefCell<State>>, ev: Ev, preferred: &str) {
    match ev {
        Ev::Tvs(Ok(tvs)) => {
            let names: Vec<String> = tvs.iter().map(|t| format!("{}  ({}, {})", t.name, t.ip, t.model)).collect();
            ui.tv_model.splice(0, ui.tv_model.n_items(), &names.iter().map(String::as_str).collect::<Vec<_>>());
            if let Some(i) = tvs.iter().position(|t| t.name == preferred) {
                ui.tv_drop.set_selected(i as u32);
            }
            ui.refresh.set_sensitive(true);
            ui.button.set_sensitive(!tvs.is_empty());
            if tvs.is_empty() {
                set_status(ui, "No TVs found", "Is the Roku on and on the same network as this computer?", false);
            } else {
                let n = tvs.len();
                set_status(ui, "Ready", &format!("{n} TV{} found. Pick one and press Start.", if n == 1 { "" } else { "s" }), false);
            }
            st.borrow_mut().tvs = tvs;
        }
        Ev::DefaultName(n) => {
            if ui.name.text().is_empty() {
                ui.name.set_text(&n);
            }
        }
        Ev::Tvs(Err(e)) => {
            ui.refresh.set_sensitive(true);
            set_status(ui, "Couldn't search for TVs", &e, false);
        }
        Ev::Line(line) => {
            append_log(ui, &line);
            if st.borrow().stopping {
                return;
            }
            if let Some(rest) = line.strip_prefix("status: ") {
                let (state, detail) = rest.split_once(' ').unwrap_or((rest, ""));
                match state {
                    "scanning" => set_status(ui, "Finding the TV", &format!("Checking {detail} is on your network…"), true),
                    "sharing" => set_status(ui, "Choose a screen", "Pick the screen to mirror in the screen-sharing dialog.", true),
                    "joining" => set_status(ui, "Connecting", &format!("Joining {detail} over Wi-Fi Direct. Accept the prompt on the TV if it asks."), true),
                    "retrying" => set_status(ui, "Connecting", "That didn't work, starting over once more…", true),
                    "waiting" => set_status(ui, "Almost there", "Connected. Waiting for the TV to start the stream…", true),
                    "streaming" => {
                        ui.button.set_sensitive(true);
                        set_status(ui, "Mirroring", &format!("Your screen is on the TV ({detail})."), false);
                    }
                    "error" => st.borrow_mut().last_error = Some(detail.to_string()),
                    _ => {}
                }
            } else if line.starts_with("p2p: join attempt") {
                ui.detail.set_text(&format!("{} The TV didn't answer yet, trying again…", line.trim_start_matches("p2p: ")));
            }
        }
        Ev::Exit(code) => {
            let (quit, err, stopping) = {
                let mut s = st.borrow_mut();
                s.child = None;
                (s.quit_after, s.last_error.take(), s.stopping)
            };
            set_running(ui, false);
            ui.button.set_sensitive(!st.borrow().tvs.is_empty());
            match err {
                Some(e) if !stopping => set_status(ui, "Couldn't mirror", &e, false),
                _ if code != 0 && !stopping => set_status(ui, "Couldn't mirror", "rokast exited unexpectedly; open Details for the log.", false),
                _ => set_status(ui, "Stopped", "Mirroring ended and the connection was cleaned up.", false),
            }
            if quit {
                ui.window.destroy();
            }
        }
    }
}

fn main() -> glib::ExitCode {
    gtk::Window::set_default_icon_name(APP_ID); // assets/rokast.svg, installed into the hicolor theme
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::empty()).build();
    app.connect_activate(build);
    app.run_with_args(&std::env::args().take(1).collect::<Vec<_>>()) // our own flags (--tray) are read in build()
}
