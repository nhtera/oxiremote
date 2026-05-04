// Agent CLI detector — polls the foreground process group of a PTY master fd
// and matches the comm name against known AI coding agent binaries.
//
// Unix-only v1. Windows support is deferred (would need GetConsoleProcessList).

use std::fmt;

#[cfg(unix)]
use std::os::unix::io::RawFd;

/// Known AI coding agent CLI binaries we can detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedAgent {
    Claude,
    Codex,
    Cursor,
    OpenCode,
}

impl fmt::Display for DetectedAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DetectedAgent::Claude => write!(f, "claude"),
            DetectedAgent::Codex => write!(f, "codex"),
            DetectedAgent::Cursor => write!(f, "cursor"),
            DetectedAgent::OpenCode => write!(f, "opencode"),
        }
    }
}

impl DetectedAgent {
    /// Parse from a lowercase name string (inverse of Display).
    #[allow(dead_code)] // used by tests and may be used by future API endpoints
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(DetectedAgent::Claude),
            "codex" => Some(DetectedAgent::Codex),
            "cursor" => Some(DetectedAgent::Cursor),
            "opencode" => Some(DetectedAgent::OpenCode),
            _ => None,
        }
    }
}

/// Trait over the comm-reading operation so unit tests can inject a mock
/// without spawning real processes.
pub trait CommReader: Send + Sync {
    /// Read the comm name (basename) for the given PID. Returns None on
    /// permission denied, process gone, or any other non-fatal error.
    fn read_comm(&self, pid: i32) -> Option<String>;
}

/// Production implementation that reads from /proc (Linux) or shells out to
/// `ps` (macOS).
pub struct OsCommReader;

impl CommReader for OsCommReader {
    fn read_comm(&self, pid: i32) -> Option<String> {
        read_comm_os(pid)
    }
}

/// Read the comm/basename for `pid` using the OS-appropriate method.
/// Returns None on any error — never panics.
#[cfg(target_os = "linux")]
fn read_comm_os(pid: i32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    match std::fs::read_to_string(&path) {
        Ok(s) => Some(s.trim().to_string()),
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
fn read_comm_os(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let comm = raw.trim();
    if comm.is_empty() {
        return None;
    }
    // `ps -o comm=` may return the full path on macOS — take only the basename.
    let basename = comm.rsplit('/').next().unwrap_or(comm);
    Some(basename.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_comm_os(_pid: i32) -> Option<String> {
    None
}

/// Detect which (if any) agent CLI is currently in the foreground process group
/// of the given PTY master file descriptor.
///
/// Returns None when: PTY is closed, no foreground process, or the foreground
/// process is not one of the known agent binaries.
#[cfg(unix)]
pub fn detect_in_pty(master_fd: RawFd, reader: &dyn CommReader) -> Option<DetectedAgent> {
    // tcgetpgrp returns -1 on error (PTY closed, invalid fd, etc.).
    let pgid = unsafe { libc::tcgetpgrp(master_fd) };
    if pgid <= 0 {
        return None;
    }
    let comm = reader.read_comm(pgid)?;
    match_comm(&comm)
}

/// Match a comm basename against the known agent binary names.
fn match_comm(comm: &str) -> Option<DetectedAgent> {
    match comm {
        "claude" => Some(DetectedAgent::Claude),
        "codex" => Some(DetectedAgent::Codex),
        "cursor" => Some(DetectedAgent::Cursor),
        "opencode" => Some(DetectedAgent::OpenCode),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockReader {
        result: Option<String>,
    }

    impl CommReader for MockReader {
        fn read_comm(&self, _pid: i32) -> Option<String> {
            self.result.clone()
        }
    }

    #[test]
    fn display_lowercases_agent_names() {
        assert_eq!(DetectedAgent::Claude.to_string(), "claude");
        assert_eq!(DetectedAgent::Codex.to_string(), "codex");
        assert_eq!(DetectedAgent::Cursor.to_string(), "cursor");
        assert_eq!(DetectedAgent::OpenCode.to_string(), "opencode");
    }

    #[test]
    fn from_name_round_trips() {
        for name in ["claude", "codex", "cursor", "opencode"] {
            let agent = DetectedAgent::from_name(name).unwrap();
            assert_eq!(agent.to_string(), name);
        }
        assert!(DetectedAgent::from_name("bash").is_none());
        assert!(DetectedAgent::from_name("").is_none());
    }

    #[test]
    fn match_comm_recognises_known_agents() {
        assert_eq!(match_comm("claude"), Some(DetectedAgent::Claude));
        assert_eq!(match_comm("codex"), Some(DetectedAgent::Codex));
        assert_eq!(match_comm("cursor"), Some(DetectedAgent::Cursor));
        assert_eq!(match_comm("opencode"), Some(DetectedAgent::OpenCode));
    }

    #[test]
    fn match_comm_ignores_unknown_names() {
        assert!(match_comm("bash").is_none());
        assert!(match_comm("node").is_none());
        assert!(match_comm("CLAUDE").is_none()); // case-sensitive
        assert!(match_comm("claude.sh").is_none()); // no suffix match
    }

    /// Simulate a PTY where the foreground process is `claude`.
    /// We cannot call `detect_in_pty` without a real fd, so we test the
    /// comm-match logic (the only logic after the libc syscall) directly.
    #[test]
    fn mock_reader_returns_claude() {
        let reader = MockReader { result: Some("claude".to_string()) };
        // Verify that a comm value of "claude" produces Claude.
        let comm = reader.read_comm(1234).unwrap();
        assert_eq!(match_comm(&comm), Some(DetectedAgent::Claude));
    }

    #[test]
    fn mock_reader_returns_none_on_permission_denied() {
        let reader = MockReader { result: None };
        assert!(reader.read_comm(99999).is_none());
    }

    #[test]
    fn mock_reader_ignores_bash_shell() {
        let reader = MockReader { result: Some("bash".to_string()) };
        let comm = reader.read_comm(42).unwrap();
        assert!(match_comm(&comm).is_none());
    }
}
