//! rokast-core: everything `rokast` does that isn't the command line. Ported module by module from
//! `src/miracast/*.py` (docs/plan-rust.md); each module says which Python file it mirrors.
pub mod audio;
pub mod capture;
pub mod conf;
pub mod joinlog;
pub mod p2p;
pub mod paths;
pub mod pyfmt;
pub mod selftest;
pub mod update;
pub mod wayland;
pub mod wfd;

use std::collections::BTreeMap;

/// The environment as a map, so everything that reads it is testable without touching the real one.
pub type Env = BTreeMap<String, String>;

/// The process environment.
pub fn env() -> Env {
    std::env::vars().collect()
}
