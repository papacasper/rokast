//! Evidence from Wi-Fi Direct join attempts that failed.
//!
//! A run with any failed attempt is saved, whether it joined in the end or not, to
//! $XDG_CACHE_HOME/rokast/join-failures/<UTC time>/ (the newest KEEP are kept):
//!   summary.json    result, target, TV state over the LAN, and per attempt: timings (s since the join began),
//!                   the TV's P2P peer entry before and after, both supplicants' status
//!   attempt-N.log   our patched supplicant's debug log for attempt N (also the attempt that joined, to compare)
//! `rokast doctor --report` prints the newest summary.
use crate::{paths, Env};
use serde_json::{json, Map, Value};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const KEEP: usize = 5;

pub fn failures_dir(env: &Env) -> PathBuf {
    paths::cache_dir(env).join("join-failures")
}

/// Saved runs, oldest first (names are UTC times, so they sort).
pub fn runs(env: &Env) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(failures_dir(env)) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

pub fn latest(env: &Env) -> Option<PathBuf> {
    runs(env).pop()
}

/// (year, month, day, hour, minute, second) in UTC for a Unix time.
pub fn utc(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    // civil_from_days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, rem / 60 % 60, rem % 60)
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub struct JoinLog {
    clock: Box<dyn Fn() -> f64>,
    pub target: Map<String, Value>,
    pub attempts: Vec<Map<String, Value>>,
    pub logs: Vec<(usize, String)>,
    pub tv: Map<String, Value>,
}

impl JoinLog {
    pub fn new(target: &Map<String, Value>) -> Self {
        let t0 = Instant::now();
        Self::with_clock(target, Box::new(move || t0.elapsed().as_secs_f64()))
    }

    /// `clock`: seconds since some fixed point (tests pass a fake one).
    pub fn with_clock(target: &Map<String, Value>, clock: Box<dyn Fn() -> f64>) -> Self {
        let mut t = Map::new();
        for k in ["name", "ip", "model", "p2p_mac"] {
            t.insert(k.into(), target.get(k).cloned().unwrap_or_else(|| json!("")));
        }
        let start = clock();
        JoinLog { clock: Box::new(move || clock() - start), target: t, attempts: vec![], logs: vec![], tv: Map::new() }
    }

    pub fn t(&self) -> f64 {
        ((self.clock)() * 100.0).round() / 100.0
    }

    fn record(&mut self, info: &Map<String, Value>, key: &str, log: &str) {
        let n = self.attempts.len() + 1;
        let mut a = info.clone();
        a.insert("attempt".into(), json!(n));
        a.insert(key.into(), json!(self.t()));
        self.attempts.push(a);
        self.logs.push((n, log.to_string()));
    }

    /// Record one failed attempt: info (timings, peer, states) and our supplicant's log text.
    pub fn failed(&mut self, info: &Map<String, Value>, log: &str) {
        self.record(info, "failed_at", log);
    }

    /// Record the attempt that joined; kept only when an earlier one failed.
    pub fn joined(&mut self, info: &Map<String, Value>, log: &str) {
        self.record(info, "joined_at", log);
    }

    /// Write the run if any attempt failed; returns its folder. Never fails loudly: evidence is best effort.
    pub fn save(&self, result: &str, env: &Env, now: Option<i64>, keep: usize) -> Option<PathBuf> {
        if !self.attempts.iter().any(|a| a.contains_key("failed_at")) {
            return None;
        }
        self.write(result, env, now.unwrap_or_else(now_secs), keep).ok()
    }

    fn write(&self, result: &str, env: &Env, now: i64, keep: usize) -> std::io::Result<PathBuf> {
        let base = failures_dir(env);
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&base)?;
        let (y, mo, d, h, mi, s) = utc(now);
        let stamp = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}");
        let mut dir = base.join(&stamp);
        let mut i = 1;
        while dir.exists() {
            i += 1;
            dir = base.join(format!("{stamp}-{i}"));
        }
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let summary = json!({
            "result": result,
            "time": format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"),
            "target": self.target, "tv": self.tv, "attempts": self.attempts,
        });
        fs::write(dir.join("summary.json"), to_json_indent1(&summary))?;
        for (n, log) in &self.logs {
            fs::write(dir.join(format!("attempt-{n}.log")), log)?;
        }
        let all = runs(env);
        for old in &all[..all.len().saturating_sub(keep)] {
            let _ = fs::remove_dir_all(old);
        }
        Ok(dir)
    }
}

/// `json.dump(x, indent=1)`.
pub fn to_json_indent1(v: &Value) -> Vec<u8> {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b" "));
    v.serialize(&mut ser).expect("JSON values always serialize");
    out
}

pub fn summary_of(dir: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(dir.join("summary.json")).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Map<String, Value> {
        json!({"name": "Living Room", "ip": "192.0.2.9", "model": "R", "p2p_mac": "aa:bb:cc:dd:ee:ff"})
            .as_object()
            .unwrap()
            .clone()
    }

    fn env_in(t: &tempfile::TempDir) -> Env {
        [("XDG_CACHE_HOME".to_string(), t.path().to_str().unwrap().to_string())].into()
    }

    fn names(env: &Env) -> Vec<String> {
        runs(env).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn test_prunes_to_newest_five() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        for i in 0..7 {
            let mut log = JoinLog::new(&target());
            log.failed(json!({"start": 0}).as_object().unwrap(), "x");
            log.save("failed", &env, Some(1_700_000_000 + i * 60), KEEP);
        }
        let kept = names(&env);
        assert_eq!(kept.len(), 5);
        assert_eq!(kept[4], "20231114-221920"); // the newest (i=6) survives
        assert_eq!(kept[0], "20231114-221520"); // i=2; i=0 and 1 pruned
    }

    #[test]
    fn test_same_second_does_not_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        for _ in 0..2 {
            let mut log = JoinLog::new(&target());
            log.failed(&Map::new(), "x");
            log.save("failed", &env, Some(1_700_000_000), KEEP);
        }
        assert_eq!(names(&env), ["20231114-221320", "20231114-221320-2"]);
    }

    #[test]
    fn test_unwritable_cache_does_not_break_the_join() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("rokast"), "").unwrap(); // a file where the folder should be
        let mut log = JoinLog::new(&target());
        log.failed(&Map::new(), "x");
        assert_eq!(log.save("failed", &env_in(&t), None, KEEP), None);
    }

    #[test]
    fn summary_contents_and_first_try_join() {
        let t = tempfile::tempdir().unwrap();
        let env = env_in(&t);
        let ticks = std::cell::Cell::new(10.0);
        let mut log = JoinLog::with_clock(
            &target(),
            Box::new(move || {
                ticks.set(ticks.get() + 1.234);
                ticks.get()
            }),
        );
        log.joined(&Map::new(), "ok");
        assert_eq!(log.save("joined on attempt 1", &env, None, KEEP), None); // first-try join keeps nothing
        log.failed(json!({"peer": {"listen_freq": "2437"}}).as_object().unwrap(), "log 2");
        let d = log.save("failed", &env, Some(0), KEEP).unwrap();
        let s = summary_of(&d).unwrap();
        assert_eq!(s["time"], "1970-01-01T00:00:00Z");
        assert_eq!(s["attempts"][1]["attempt"], 2);
        assert_eq!(s["attempts"][1]["peer"]["listen_freq"], "2437");
        assert_eq!(s["target"]["name"], "Living Room");
        assert_eq!(fs::read_to_string(d.join("attempt-2.log")).unwrap(), "log 2");
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        assert_eq!(utc(951_782_400), (2000, 2, 29, 0, 0, 0));
    }
}
