//! Stopping `rokast start`: SIGTERM/SIGINT set a flag that every wait polls (Python turns SIGTERM into
//! KeyboardInterrupt). The teardown that follows runs whatever the flag says.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

/// Why a step of `start` ended early: stopped by the user (not an error), or an error to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Stop,
    Err(String),
}

impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Err(e)
    }
}

impl From<&str> for Fail {
    fn from(e: &str) -> Self {
        Fail::Err(e.into())
    }
}

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// SIGTERM and SIGINT set the flag instead of killing the process.
pub fn install() {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

pub fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Err(Stop) once a stop was asked for.
pub fn check() -> Result<(), Fail> {
    if stopped() {
        Err(Fail::Stop)
    } else {
        Ok(())
    }
}

/// Sleep `d`, waking early (Err(Stop)) when a stop is asked for.
pub fn sleep(d: Duration) -> Result<(), Fail> {
    let end = Instant::now() + d;
    loop {
        check()?;
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}
