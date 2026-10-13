//! `audio_hold <AUDIO value>`: route sound like a playing session does, until SIGTERM/SIGINT, then restore.
//! Drives tests/test_audio_live.sh against the real sound server (no TV needed).
use rokast_core::audio::{self, Router};
use rokast_core::stop;
use std::time::Duration;

fn main() {
    let value = std::env::args().nth(1).unwrap_or_else(|| "tv".into());
    let env = rokast_core::env();
    stop::install();
    if audio::recover(&env) {
        println!("recovered");
    }
    let mut r = Router::new(&value, &env).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    match r.route() {
        Ok(s) => println!("source: {}", s.unwrap_or_else(|| "none".into())),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
    while stop::sleep(Duration::from_secs(60)).is_ok() {}
    r.restore();
    println!("restored");
}
