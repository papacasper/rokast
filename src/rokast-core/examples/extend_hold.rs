//! `extend_hold <W> <H> [side]`: add a WxH screen beside the primary monitor like a playing SCREEN=extend session,
//! until SIGTERM/SIGINT, then remove it. Drives tests/test_extend_live.sh against the real display (no TV needed).
use rokast_core::capture;
use rokast_core::extend::{self, Extender};
use rokast_core::stop;
use std::time::Duration;

fn main() {
    let arg = |i: usize, d: u32| std::env::args().nth(i).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (arg(1, 1920), arg(2, 1080));
    let side = std::env::args().nth(3).unwrap_or_default();
    let env = rokast_core::env();
    let display = env.get("DISPLAY").cloned().unwrap_or_else(|| ":0".into());
    stop::install();
    if extend::recover(&env) {
        println!("recovered");
    }
    let mon =
        capture::monitor_geometry(&capture::x11_monitors(&display, &env), "").ok().flatten().unwrap_or_else(|| {
            eprintln!("no monitor");
            std::process::exit(2)
        });
    let mut x = Extender::new("extend", &side, &display, &env).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    match x.apply(&mon, w, h) {
        Ok(Some((g, kind))) => println!("extended: {} {}x{}+{}+{} {kind}", g.name, g.width, g.height, g.x, g.y),
        Ok(None) => println!("unchanged"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
    while stop::sleep(Duration::from_secs(60)).is_ok() {}
    x.restore();
    println!("restored");
}
