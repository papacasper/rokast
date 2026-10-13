//! `rokast start [name]`: find the TV, join its Wi-Fi Direct group, stream until stopped.
use crate::{die, Ctx};
use rokast_core::capture::{self, Monitor, Opts, Pipeline};
use rokast_core::conf::Conf;
use rokast_core::link::{self, Radio, Wpa};
use rokast_core::rtsp::{self, Source};
use rokast_core::session::{self, Link};
use rokast_core::stop::{self, Fail};
use rokast_core::wayland::ScreenCast;
use rokast_core::wfd::Negotiated;
use rokast_core::{lan, p2p::Roku, update};
use std::process::exit;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Machine-readable progress line (the UI keys off these):
/// scanning, sharing (Wayland screen picker), joining, retrying, waiting, streaming, stopped, error.
fn status(state: &str, detail: &str) {
    println!("status: {state}{}", if detail.is_empty() { String::new() } else { format!(" {detail}") });
}

fn say(m: &str) {
    println!("{m}");
}

struct RealLink<'a> {
    radio: Wpa,
    dev: String,
    target: Roku,
    conf: &'a Conf,
    geometry: Option<Monitor>,
    cast: Option<&'a ScreenCast>,
}

impl Link for RealLink<'_> {
    fn connect(&mut self) -> Result<String, Fail> {
        let tries_timeout = Duration::from_secs(15);
        link::connect(&mut self.radio, &self.dev, &self.target, &self.conf["NAME"], 4, tries_timeout, &say)
    }

    fn stream(&mut self, iface: &str, played: &mut bool) -> Result<(), Fail> {
        // must listen BEFORE the lease: the TV connects ~0.5 s after it
        let l = rtsp::listen(7236, Some(iface), "").map_err(|e| format!("wfd: listen on {iface}:7236: {e}"))?;
        say(&format!("wfd: listening on {iface}:7236 for the sink"));
        let (tx, rx) = channel();
        let (i, env) = (iface.to_string(), self.radio.env.clone());
        std::thread::spawn(move || {
            let _ = tx.send(link::dhcp(&i, 15, &env));
        });
        let mut lease: Option<Receiver<_>> = Some(rx);
        let mut deadline: Option<Instant> = None;
        let mut poll = |playing: bool| -> Result<(), Fail> {
            stop::check()?;
            if let Some(rx) = &lease {
                match rx.try_recv() {
                    Ok(Ok(ip)) => {
                        say(&format!("link up: {iface} {ip}, waiting for the TV to connect"));
                        status("waiting", "");
                        deadline = Some(Instant::now() + Duration::from_secs(20));
                        lease = None;
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => return Err("dhcp: no answer".into()),
                }
            }
            if !playing && deadline.is_some_and(|d| Instant::now() >= d) {
                return Err("the TV joined but never opened RTSP to us (screen-mirroring prompt declined?)".into());
            }
            Ok(())
        };
        let mut ff: Option<Pipeline> = None;
        let (conf, geometry, env, cast) = (self.conf, self.geometry.clone(), &self.radio.env, self.cast);
        let mut on_play = |n: &Negotiated, ip: &str, port: u16| -> Result<(), String> {
            let audio = conf["AUDIO"] == "1";
            let opts = Opts {
                display: conf["DISPLAY"].clone(),
                encoder: conf["ENCODER"].clone(),
                bitrate: conf["BITRATE"].clone(),
                audio: audio && n.audio == "AAC",
                monitor: capture::default_monitor(),
                source: if cast.is_some() { "raw" } else { "x11" }.into(),
                out: None,
                seconds: None,
                geometry: geometry.clone(),
            };
            let cmd = capture::build_cmd(n, Some(ip), Some(port), &opts);
            let feeder =
                cast.map(|c| (rokast_core::wayland::gst_cmd(c.raw_fd(), c.node, n.width, n.height, n.fps), c.raw_fd()));
            if let Some((argv, _)) = &feeder {
                say(&format!("gst: {}", argv.join(" ")));
            }
            say(&format!("ffmpeg: {}", cmd.join(" ")));
            *played = true;
            let feeder = feeder.as_ref().map(|(argv, fd)| (argv.as_slice(), *fd));
            ff = Some(capture::start(&cmd, &conf["DISPLAY"], env, feeder).map_err(|e| format!("ffmpeg: {e}"))?);
            let a = if audio { n.audio.as_str() } else { "no audio" };
            status("streaming", &format!("{}x{}@{} {a}", n.width, n.height, n.fps));
            Ok(())
        };
        let prefs = conf["RES"].split(',').map(|p| p.trim().to_string()).collect();
        let mut src = Source::new(prefs, &say);
        let res = src.serve(&l, &mut poll, &mut on_play);
        if let Some(mut p) = ff {
            p.terminate(Duration::from_secs(5));
        }
        res
    }

    fn disconnect(&mut self) {
        self.radio.disconnect(&self.dev, None);
    }
}

fn steps(ctx: &Ctx, conf: &mut Conf, mode: &str, name: &str, dev: &mut Option<String>) -> Result<(), Fail> {
    status("scanning", name);
    let rokus = lan::lan_rokus(None, &ctx.env)?;
    let target = rokast_core::p2p::pick(&rokus, name)?.clone();
    say(&format!("target: {} ({}, p2p {})", target.name, target.ip, target.p2p_mac));
    stop::check()?;
    let enc = capture::pick_encoder(&conf["ENCODER"])?; // probe before joining: the TV's RTSP window is short
    conf.insert("ENCODER".into(), enc.clone());
    say(&format!("encoder: {enc}"));
    stop::check()?;
    // unknown MONITOR fails here, before joining; on Wayland the portal picks
    let geometry = match mode {
        "x11" => capture::monitor_geometry(&capture::x11_monitors(&conf["DISPLAY"], &ctx.env), &conf["MONITOR"])?,
        _ => None,
    };
    let shown = geometry
        .as_ref()
        .map(|g| format!(" monitor {} {}x{}+{}+{}", g.name, g.width, g.height, g.x, g.y))
        .unwrap_or_default();
    say(&format!("capture: {mode}{shown}, name on TV: {}", conf["NAME"]));
    let mut cast = None;
    if mode == "wayland" {
        // the portal may ask the user to pick a screen: do it before joining
        status("sharing", "");
        let mut c = ScreenCast::new()?;
        c.open(&ctx.env)?;
        say(&format!("pipewire: node {}", c.node));
        cast = Some(c);
    }
    let d = lan::p2p_dev_iface(&ctx.env)?;
    *dev = Some(d.clone());
    stop::check()?;
    let radio = Wpa { env: ctx.env.clone(), root: ctx.root.clone() };
    let mut real = RealLink { radio, dev: d, target: target.clone(), conf, geometry, cast: cast.as_ref() };
    session::join_and_stream(&mut real, &target.name, &say, 2)
}

pub fn run(ctx: &Ctx, args: &[String]) -> ! {
    let mut conf = ctx.load_conf();
    let name = args.get(1).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| conf["TARGET"].clone());
    if name.is_empty() {
        status("error", "no TV selected");
        die(&format!("no target: `rokast start \"<name>\"` or set TARGET in {} (see `rokast list`)", conf["_file"]));
    }
    let mode = ctx.capture_mode(&conf);
    stop::install();
    let session = update::hold_session(&ctx.env); // `rokast update` refuses while this is held
    let mut dev = None;
    let res = steps(ctx, &mut conf, mode, &name, &mut dev);
    let failed = match res {
        Ok(()) => None,
        Err(Fail::Stop) => {
            say("stopping");
            None
        }
        Err(Fail::Err(e)) => {
            status("error", &e);
            Some(e)
        }
    };
    if let Some(d) = dev {
        Wpa { env: ctx.env.clone(), root: ctx.root.clone() }.disconnect(&d, None);
    }
    drop(session);
    if let Some(e) = failed {
        die(&format!("error: {e}"));
    }
    status("stopped", "");
    exit(0)
}
