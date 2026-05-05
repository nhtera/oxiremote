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
                // Ownership check: PIDs are recycled on busy systems, so the
                // PID we wrote on the last shutdown may now belong to an
                // unrelated process (sshd, a build job, the user's editor).
                // Killing it would be a serious foot-gun — verify that the
                // process is actually our own binary before sending signals.
                if pid != me && process_alive(pid) && pid_is_oxiremote(pid) {
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

/// Best-effort fallback when bind() fails with AddrInUse but `acquire()` did
/// not catch a stale process — usually because the prior instance crashed
/// without writing a pidfile, or wrote one in a different data_dir (e.g. an
/// older release with the pre-USERPROFILE env-fallback bug). Scans the
/// process table for live `oxiremote` processes other than ourselves and
/// terminates them. Returns the number of processes killed.
pub fn kill_other_oxiremote_processes() -> u32 {
    let me = std::process::id() as i32;
    let mut killed = 0;
    for pid in list_oxiremote_pids() {
        if pid == me || !process_alive(pid) || !pid_is_oxiremote(pid) {
            continue;
        }
        let _ = kill_process(pid, false);
        std::thread::sleep(Duration::from_millis(500));
        if process_alive(pid) {
            let _ = kill_process(pid, true);
            std::thread::sleep(Duration::from_millis(200));
        }
        killed += 1;
    }
    killed
}

#[cfg(target_os = "linux")]
fn list_oxiremote_pids() -> Vec<i32> {
    std::process::Command::new("pgrep")
        .args(["-x", "oxiremote"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn list_oxiremote_pids() -> Vec<i32> {
    std::process::Command::new("pgrep")
        .args(["-x", "oxiremote"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn list_oxiremote_pids() -> Vec<i32> {
    Vec::new()
}

#[cfg(windows)]
fn list_oxiremote_pids() -> Vec<i32> {
    // tasklist CSV row: "oxiremote.exe","1234","Console","1","45,000 K"
    let out = std::process::Command::new("tasklist")
        .args([
            "/FI",
            "IMAGENAME eq oxiremote.exe",
            "/FO",
            "CSV",
            "/NH",
        ])
        .output();
    let Ok(o) = out else { return Vec::new() };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split(',');
            parts.next()?; // image name
            let pid_field = parts.next()?.trim().trim_matches('"');
            pid_field.parse().ok()
        })
        .collect()
}

/// Verify the named PID is actually an oxiremote process. Belt-and-braces
/// against PID reuse: the agent's `agent.pid` file may point at a recycled
/// PID owned by an unrelated process after a crash + reboot. Conservative on
/// failure — if we can't determine the binary name, return `false` and skip
/// the kill rather than risk hitting an innocent process.
///
/// Linux: read `/proc/{pid}/comm`.
/// macOS: invoke `ps -p {pid} -o comm=` (libproc would need a C dep).
/// Windows: handled in the windows-cfg block; tasklist already filters by PID
/// so the kill path is naturally narrower.
#[cfg(target_os = "linux")]
fn pid_is_oxiremote(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim() == "oxiremote")
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn pid_is_oxiremote(pid: i32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            // ps prints the full path on macOS; match on the basename only so
            // running from `~/.cargo/bin/oxiremote` and `target/release/oxiremote`
            // both verify cleanly.
            s.trim()
                .rsplit('/')
                .next()
                .map(|n| n == "oxiremote")
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn pid_is_oxiremote(_pid: i32) -> bool {
    // Other Unix targets — we can't introspect cheaply without a C dep, so
    // skip the kill rather than risk hitting the wrong process.
    false
}

#[cfg(windows)]
fn pid_is_oxiremote(pid: i32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.to_ascii_lowercase().contains("oxiremote")
        }
        Err(_) => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Belt-and-braces against PID reuse: after a crash + reboot the agent's
    /// pid file may point at a recycled PID owned by sshd / a build job /
    /// the user's editor. `pid_is_oxiremote` MUST return false for those so
    /// the kill path skips them.
    #[test]
    fn skips_kill_when_pid_belongs_to_other_process() {
        // PID 1 is init/launchd — never our binary. The guard MUST reject
        // it regardless of liveness check (on macOS unprivileged `kill -0 1`
        // returns failure, but a privileged or recycled probe could pass).
        assert!(
            !pid_is_oxiremote(1),
            "init/launchd must not be misidentified as oxiremote — \
             would cause kill -TERM 1 attempts in the wild"
        );

        // The test harness binary is called `oxiremote-<hash>` (not bare
        // `oxiremote`), so the guard correctly rejects our own PID too.
        // Pinning this protects cargo tests from killing themselves on a
        // hypothetical stale pid file pointing at the harness PID.
        let me = std::process::id() as i32;
        assert!(process_alive(me), "the test harness should see itself as alive");
        assert!(
            !pid_is_oxiremote(me),
            "test harness binary should not match the bare `oxiremote` name"
        );
    }

    /// A plainly-bogus PID must report as not-alive. Defensive — `kill -0` on
    /// a free PID returns failure, so the guard short-circuits before
    /// pid_is_oxiremote ever runs.
    #[test]
    fn process_alive_false_for_unused_pid() {
        // 2^31 - 1 is the kernel max PID; no process should hold it.
        assert!(!process_alive(i32::MAX));
    }
}
