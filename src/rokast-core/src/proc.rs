//! Running other programs: Python's subprocess.run(capture_output=True, timeout=...) and shutil.which.
use crate::Env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status.success()
    }
}

/// `shutil.which`: the first executable `name` on $PATH (or `name` itself if it has a slash).
pub fn which(name: &str, env: &Env) -> Option<PathBuf> {
    if name.contains('/') {
        return crate::paths::is_executable(Path::new(name)).then(|| name.into());
    }
    let path = env.get("PATH").map(String::as_str).unwrap_or("/usr/local/bin:/usr/bin:/bin");
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file() && crate::paths::is_executable(p))
}

/// Wait for `child` at most `timeout`; None (and the child killed) if it ran over.
pub fn wait_for(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let end = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Exit code as Python's returncode: the code, or -signal.
pub fn returncode(st: &ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    st.code().unwrap_or_else(|| -st.signal().unwrap_or(0))
}

/// Run `argv` with output captured; extra environment in `envs`. None if it can't start or runs past `timeout`.
pub fn run(argv: &[&str], envs: &[(&str, &str)], timeout: Duration) -> Option<Output> {
    let mut child = Command::new(argv.first()?)
        .args(&argv[1..])
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let reader = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        })
    };
    let out = reader(Box::new(child.stdout.take()?));
    let err = reader(Box::new(child.stderr.take()?));
    let status = wait_for(&mut child, timeout);
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    Some(Output { status: status?, stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_and_timeout() {
        let o = run(&["sh", "-c", "echo out; echo err >&2; exit 3"], &[("X", "1")], Duration::from_secs(5)).unwrap();
        assert_eq!((o.stdout.as_str(), o.stderr.as_str(), returncode(&o.status)), ("out\n", "err\n", 3));
        let env = run(&["sh", "-c", "echo $ROKAST_T"], &[("ROKAST_T", "v")], Duration::from_secs(5)).unwrap();
        assert_eq!(env.stdout, "v\n");
        assert!(run(&["sleep", "5"], &[], Duration::from_millis(100)).is_none());
        assert!(run(&["/nonexistent/x"], &[], Duration::from_secs(1)).is_none());
    }

    #[test]
    fn test_which() {
        let env: Env = [("PATH".to_string(), "/nonexistent:/bin:/usr/bin".to_string())].into();
        assert!(which("sh", &env).is_some());
        assert_eq!(which("no-such-program-rokast", &env), None);
    }
}
