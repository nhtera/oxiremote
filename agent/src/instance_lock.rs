// Advisory file lock for single-instance enforcement.
//
// Uses flock(2) on Unix and LockFileEx on Windows — the lock is held for the
// entire process lifetime via OwnedFd / OwnedHandle. This prevents the
// PID-file kill-and-overwrite race that caused guaranteed respawn loops when
// combined with launchd KeepAlive=Crashed: launchd could fire a second instance
// before the first finished startup, the second killed the first, and the loser
// exited non-zero, triggering another respawn.
//
// Lock failure (another live instance holds it) → exit(0) so the OS supervisor
// does NOT count it as a crash and respawn again.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub struct InstanceLock {
    _lock_file: LockFile,
}

impl InstanceLock {
    /// Acquire the advisory lock. If another instance already holds it, exits
    /// the process with code 0 (graceful — does not trigger crash respawn).
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("create data dir for lock file")?;
        let lock_path = data_dir.join("agent.lock");

        match LockFile::acquire(&lock_path) {
            Ok(lock_file) => Ok(Self { _lock_file: lock_file }),
            Err(LockError::AlreadyLocked) => {
                // Another live instance is running — exit cleanly so launchd /
                // systemd / Task Scheduler does not treat this as a crash.
                tracing::info!("another oxiremote instance is running; exiting");
                std::process::exit(0);
            }
            Err(LockError::Io(e)) => {
                Err(anyhow::anyhow!("instance lock I/O error: {e}"))
            }
        }
    }
}

// ------------------------------------------------------------------
// Platform lock implementations
// ------------------------------------------------------------------

enum LockError {
    AlreadyLocked,
    Io(std::io::Error),
}

impl From<std::io::Error> for LockError {
    fn from(e: std::io::Error) -> Self {
        LockError::Io(e)
    }
}

// ------------------------------------------------------------------
// Unix: flock(2) advisory lock held via OwnedFd
// ------------------------------------------------------------------

#[cfg(unix)]
struct LockFile {
    _fd: std::os::unix::io::OwnedFd,
    path: PathBuf,
}

#[cfg(unix)]
impl LockFile {
    fn acquire(path: &Path) -> std::result::Result<Self, LockError> {
        use std::os::unix::io::IntoRawFd;
        use std::os::unix::io::FromRawFd;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(LockError::Io)?;

        let fd = file.into_raw_fd();

        // LOCK_EX | LOCK_NB: exclusive, non-blocking.
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            // EWOULDBLOCK / EAGAIN means another process holds the lock.
            if err.kind() == std::io::ErrorKind::WouldBlock {
                unsafe { libc::close(fd) };
                return Err(LockError::AlreadyLocked);
            }
            unsafe { libc::close(fd) };
            return Err(LockError::Io(err));
        }

        // Write our PID into the lock file for observability (not used for logic).
        let pid = std::process::id().to_string();
        unsafe {
            // Truncate then write — best-effort, ignore errors.
            libc::ftruncate(fd, 0);
            let _ = libc::write(fd, pid.as_ptr() as *const libc::c_void, pid.len());
        }

        let owned = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(fd) };
        Ok(Self { _fd: owned, path: path.to_path_buf() })
    }
}

#[cfg(unix)]
impl Drop for LockFile {
    fn drop(&mut self) {
        // flock is released automatically when the fd closes (OwnedFd drop).
        // Remove the lock file so the next start doesn't see stale bytes.
        let _ = std::fs::remove_file(&self.path);
    }
}

// ------------------------------------------------------------------
// Windows: LockFileEx advisory lock held via OwnedHandle
// ------------------------------------------------------------------

#[cfg(windows)]
struct LockFile {
    _handle: std::os::windows::io::OwnedHandle,
    path: PathBuf,
}

#[cfg(windows)]
impl LockFile {
    fn acquire(path: &Path) -> std::result::Result<Self, LockError> {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        use windows_sys::Win32::Foundation::{HANDLE, FALSE};
        use windows_sys::Win32::Storage::FileSystem::{
            LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
        };

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(LockError::Io)?;

        let handle: HANDLE = file.into_raw_handle() as HANDLE;

        // OVERLAPPED zeroed — required by LockFileEx even when offset is 0.
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };

        let ret = unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut overlapped,
            )
        };

        if ret == FALSE {
            let err = std::io::Error::last_os_error();
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            // ERROR_LOCK_VIOLATION (33) means already locked.
            if err.raw_os_error() == Some(33) {
                return Err(LockError::AlreadyLocked);
            }
            return Err(LockError::Io(err));
        }

        // Write PID for observability.
        let pid = std::process::id().to_string();
        let _ = std::fs::write(path, &pid);

        let owned = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle as _) };
        Ok(Self { _handle: owned, path: path.to_path_buf() })
    }
}

#[cfg(windows)]
impl Drop for LockFile {
    fn drop(&mut self) {
        // Lock released when OwnedHandle drops (closes the HANDLE).
        let _ = std::fs::remove_file(&self.path);
    }
}

// ------------------------------------------------------------------
// Fallback: platforms that are neither unix nor windows
// ------------------------------------------------------------------

#[cfg(not(any(unix, windows)))]
struct LockFile {
    path: PathBuf,
}

#[cfg(not(any(unix, windows)))]
impl LockFile {
    fn acquire(path: &Path) -> std::result::Result<Self, LockError> {
        // No advisory lock available — best-effort PID file only.
        std::fs::write(path, std::process::id().to_string()).map_err(LockError::Io)?;
        Ok(Self { path: path.to_path_buf() })
    }
}

#[cfg(not(any(unix, windows)))]
impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ------------------------------------------------------------------
// Belt-and-braces sweep (preserved from prior implementation)
// ------------------------------------------------------------------

/// Best-effort fallback when bind() fails with AddrInUse but `acquire()` did
/// not catch a stale process. Scans the process table for live `oxiremote`
/// processes other than ourselves and terminates them. Returns the number of
/// processes killed.
///
/// With flock-based locking this path should rarely trigger, but it is
/// preserved as a safety net for edge cases (e.g. lock file on a networked
/// volume that doesn't honour flock semantics).
pub fn kill_other_oxiremote_processes() -> u32 {
    let me = std::process::id() as i32;
    let mut killed = 0;
    for pid in list_oxiremote_pids() {
        if pid == me || !process_alive(pid) || !pid_is_oxiremote(pid) {
            continue;
        }
        let _ = kill_process(pid, false);
        std::thread::sleep(std::time::Duration::from_millis(500));
        if process_alive(pid) {
            let _ = kill_process(pid, true);
            std::thread::sleep(std::time::Duration::from_millis(200));
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

/// Conservative PID ownership check — if we can't verify the binary name,
/// skip the kill rather than risk hitting an innocent process.
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

    /// The PID-based ownership guard (used by the sweep fallback) must reject
    /// pid 1 (init/launchd) and the test harness binary itself.
    #[test]
    fn skips_kill_when_pid_belongs_to_other_process() {
        assert!(
            !pid_is_oxiremote(1),
            "init/launchd must not be misidentified as oxiremote"
        );

        let me = std::process::id() as i32;
        assert!(process_alive(me), "the test harness should see itself as alive");
        assert!(
            !pid_is_oxiremote(me),
            "test harness binary should not match the bare `oxiremote` name"
        );
    }

    #[test]
    fn process_alive_false_for_unused_pid() {
        assert!(!process_alive(i32::MAX));
    }
}
