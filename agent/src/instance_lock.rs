// PID-file based instance lock. On acquire, an existing live process listed in
// `<data_dir>/agent.pid` is terminated so the new instance can bind 8787
// without `Address already in use`.
//
// Cross-platform without extra crates: shell out to `kill`/`taskkill`. The
// latency cost (~1ms) is fine at startup.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

pub struct InstanceLock {
    pid_path: PathBuf,
}

impl InstanceLock {
    /// Read the PID file. If a live process exists, kill it (gracefully, then
    /// hard) so the port is freed. Then write our own PID.
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("create data dir for pid file")?;
        let pid_path = data_dir.join("agent.pid");

        if let Ok(contents) = std::fs::read_to_string(&pid_path)
            && let Ok(pid) = contents.trim().parse::<i32>() {
                let me = std::process::id() as i32;
                if pid != me && process_alive(pid) {
                    let _ = kill_process(pid, false);
                    std::thread::sleep(Duration::from_millis(800));
                    if process_alive(pid) {
                        let _ = kill_process(pid, true);
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }

        std::fs::write(&pid_path, std::process::id().to_string())
            .context("write pid file")?;
        Ok(Self { pid_path })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.pid_path);
    }
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn kill_process(pid: i32, force: bool) -> bool {
    let sig = if force { "-KILL" } else { "-TERM" };
    std::process::Command::new("kill")
        .arg(sig)
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_alive(pid: i32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains(&pid.to_string())
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
fn kill_process(pid: i32, force: bool) -> bool {
    let mut cmd = std::process::Command::new("taskkill");
    if force {
        cmd.arg("/F");
    }
    cmd.args(["/PID", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
