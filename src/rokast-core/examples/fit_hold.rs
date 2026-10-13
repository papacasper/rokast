//! `fit_hold <W> <H>`: switch the primary monitor's output to WxH like a playing FIT=match session, until
//! SIGTERM/SIGINT, then restore. Drives tests/test_fit_live.sh against the real display (no TV needed).
use rokast_core::capture;
use rokast_core::fit::{self, Fitter};
use rokast_core::stop;
use std::time::Duration;

fn main() {
    let arg = |i: usize, d: u32| std::env::args().nth(i).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (w, h) = (arg(1, 1920), arg(2, 1080));
    let env = rokast_core::env();
    let display = env.get("DISPLAY").cloned().unwrap_or_else(|| ":0".into());
    stop::install();
    if fit::recover(&env) {
        println!("recovered");
    }
    let mon = capture::monitor_geometry(&capture::x11_monitors(&display, &env), "").ok().flatten().unwrap_or_else(|| {
        eprintln!("no monitor");
        std::process::exit(2)
    });
    let mut f = Fitter::new("match", &display, &env).expect("match is valid");
    match f.apply(&mon, w, h) {
        Ok(Some(g)) => println!("switched: {} {}x{}+{}+{}", g.name, g.width, g.height, g.x, g.y),
        Ok(None) => println!("unchanged"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
    while stop::sleep(Duration::from_secs(60)).is_ok() {}
    f.restore();
    println!("restored");
}
