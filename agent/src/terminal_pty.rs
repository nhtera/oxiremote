use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::warn;

use crate::db::now_ts;
use crate::events::{AgentEvent, EventBus};
use crate::terminal_buffer::{RingBuffer, Snapshot};

pub const MAX_TERMINAL_SESSIONS_PER_USER: usize = 10;
pub const WS_MAX_TEXT_BYTES: usize = 64 * 1024;

/// Threshold for considering a session "active" (recently produced output).
const ACTIVE_THRESHOLD_MS: u128 = 250;

/// Default ring buffer capacity (bytes). Override via `OXI_PTY_BUFFER_BYTES`.
const DEFAULT_BUFFER_BYTES: usize = 2 * 1024 * 1024;

/// How often to poll the PTY foreground process for agent detection.
const AGENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

fn buffer_capacity_bytes() -> usize {
    std::env::var("OXI_PTY_BUFFER_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_BUFFER_BYTES)
}

pub struct TerminalSession {
    pub owner_session_id: String,
    pub writer: std::sync::Mutex<Box<dyn std::io::Write + Send>>,
    pub output_tx: broadcast::Sender<WsOut>,
    pub child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    pub master: std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    /// Tracks the last time output was received from the PTY.
    pub last_activity: Arc<std::sync::Mutex<Instant>>,
    /// Bounded scrollback with monotonic seq. Shared with reader thread.
    pub buffer: Arc<std::sync::Mutex<RingBuffer>>,
    /// Latest seq published to the buffer. Lock-free read for API.
    pub last_seq: Arc<AtomicU64>,
    /// Raw file descriptor for the PTY master — used by the agent detector
    /// to call tcgetpgrp(). Only valid on Unix; -1 on other platforms.
    #[cfg(unix)]
    pub master_fd: std::os::unix::io::RawFd,
}

impl TerminalSession {
    /// Returns "exited" | "active" | "idle" based on DB status and recent activity.
    pub fn state(&self, db_status: &str) -> &'static str {
        if db_status == "exited" || db_status == "closed" || db_status == "dead" {
            return "exited";
        }
        let elapsed = self
            .last_activity
            .lock()
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(u128::MAX);
        if elapsed <= ACTIVE_THRESHOLD_MS {
            "active"
        } else {
            "idle"
        }
    }

    /// Produce a replay snapshot for a (re)attaching client.
    pub fn snapshot_since(&self, last_seq: Option<u64>) -> Snapshot {
        self.buffer
            .lock()
            .map(|b| b.snapshot_since(last_seq))
            .unwrap_or(Snapshot {
                from_seq: 0,
                to_seq: 0,
                data: String::new(),
            })
    }

    /// Read the last `n` lines from the ring buffer for push payload.
    pub fn last_n_lines(&self, n: usize) -> Vec<String> {
        let snap = self.buffer
            .lock()
            .map(|b| b.snapshot_since(None))
            .unwrap_or(Snapshot { from_seq: 0, to_seq: 0, data: String::new() });
        if snap.data.is_empty() {
            return Vec::new();
        }
        snap.data
            .lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(str::to_string)
            .collect()
    }
}

#[derive(Serialize)]
pub struct TerminalSessionMeta {
    pub id: String,
    pub name: Option<String>,
    pub host_id: String,
    pub state: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub cols: i64,
    pub rows: i64,
    pub status: String,
    pub exit_code: Option<i64>,
    pub last_seq: u64,
    pub buffer_bytes: u64,
    pub attached: bool,
    pub detected_agent: Option<String>,
    pub notify_on_agent_end: bool,
}

#[derive(Deserialize)]
pub struct RenameTerminalRequest {
    pub name: String,
}

#[derive(Clone, Serialize)]
#[serde(tag = "t")]
pub enum WsOut {
    /// Live output chunk with seq number. Client stores `seq` for reattach.
    #[serde(rename = "chunk")]
    Chunk { seq: u64, data: String },
    /// Replay snapshot delivered right after `attach`.
    #[serde(rename = "snapshot")]
    Snapshot {
        from_seq: u64,
        to_seq: u64,
        data: String,
    },
    #[serde(rename = "exit")]
    Exit { code: Option<i64> },
    #[serde(rename = "state")]
    State { state: String },
    #[serde(rename = "renamed")]
    Renamed { name: String },
    /// Agent detection state change — client renders badge/toggle.
    #[serde(rename = "agent_detected")]
    AgentDetected { agent_name: String },
    /// Agent ended — client clears badge.
    #[serde(rename = "agent_ended")]
    AgentEnded { agent_name: String },
}

#[derive(Deserialize)]
#[serde(tag = "t")]
pub enum WsIn {
    /// First frame sent by client on connect. `last_seq=None` → full replay.
    #[serde(rename = "attach")]
    Attach { last_seq: Option<u64> },
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Deserialize)]
pub struct CreateTerminalSessionRequest {
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Serialize)]
pub struct CreateTerminalSessionResponse {
    pub id: String,
}

#[derive(Deserialize)]
pub struct ResizeTerminalRequest {
    pub cols: u16,
    pub rows: u16,
}

pub fn build_default_command(command: Option<&[String]>) -> Vec<String> {
    if let Some(cmd) = command
        && !cmd.is_empty() {
            return cmd.to_vec();
        }
    #[cfg(target_os = "macos")]
    {
        vec!["/bin/zsh".into(), "-l".into()]
    }
    #[cfg(target_os = "windows")]
    {
        // Windows shells have no `-l` (login) concept. PowerShell ships with
        // Win10+ and is on PATH at %SystemRoot%\System32\WindowsPowerShell\v1.0;
        // CommandBuilder resolves it via PATH. -NoLogo skips the banner.
        vec!["powershell.exe".into(), "-NoLogo".into()]
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        vec![shell, "-l".into()]
    }
}

/// Spawn the 5s agent-detection poll loop for a PTY session. On each tick,
/// calls `detect_in_pty` and broadcasts `SessionAgentDetected/Ended` on state
/// changes. Also updates the `detected_agent` column in SQLite.
///
/// Only compiled on Unix — on other platforms this is a no-op stub.
#[cfg(unix)]
pub fn spawn_agent_detector(
    session_id: String,
    master_fd: std::os::unix::io::RawFd,
    db_path: PathBuf,
    event_bus: Arc<EventBus>,
    output_tx: broadcast::Sender<WsOut>,
) {
    use crate::agent_detector::{detect_in_pty, OsCommReader};

    tokio::spawn(async move {
        let reader = OsCommReader;
        let mut prev_agent: Option<String> = None;
        // Timestamp when the current agent was first detected (for duration calc).
        let mut agent_started_at: Option<std::time::Instant> = None;
        let mut interval = tokio::time::interval(AGENT_POLL_INTERVAL);
        // Skip the first tick (fires immediately) to avoid a false positive on
        // the shell startup phase before any real agent has been launched.
        interval.tick().await;

        loop {
            interval.tick().await;

            let current = detect_in_pty(master_fd, &reader).map(|a| a.to_string());

            match (&prev_agent, &current) {
                // No change — nothing to do.
                (a, b) if a == b => {}

                // Agent appeared (or changed to a different agent).
                (_, Some(name)) => {
                    let name = name.clone();
                    agent_started_at = Some(std::time::Instant::now());

                    // Update DB.
                    let db = db_path.clone();
                    let id = session_id.clone();
                    let agent_col = name.clone();
                    if let Err(err) = tokio::task::spawn_blocking(move || {
                        let conn = Connection::open(&db)?;
                        conn.execute(
                            "UPDATE terminal_sessions SET detected_agent=?2 WHERE terminal_session_id=?1",
                            params![id, agent_col],
                        )?;
                        Ok::<_, anyhow::Error>(())
                    })
                    .await
                    {
                        warn!(session=%session_id, error=%err, "agent detect db update failed");
                    }

                    // Broadcast to event bus + WS subscribers.
                    event_bus.send(AgentEvent::SessionAgentDetected {
                        session_id: session_id.clone(),
                        agent_name: name.clone(),
                    });
                    let _ = output_tx.send(WsOut::AgentDetected { agent_name: name.clone() });
                    prev_agent = Some(name);
                }

                // Agent disappeared.
                (Some(prev_name), None) => {
                    let duration_ms = agent_started_at
                        .take()
                        .map(|t| t.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    let name = prev_name.clone();

                    // Clear DB column.
                    let db = db_path.clone();
                    let id = session_id.clone();
                    if let Err(err) = tokio::task::spawn_blocking(move || {
                        let conn = Connection::open(&db)?;
                        conn.execute(
                            "UPDATE terminal_sessions SET detected_agent=NULL WHERE terminal_session_id=?1",
                            params![id],
                        )?;
                        Ok::<_, anyhow::Error>(())
                    })
                    .await
                    {
                        warn!(session=%session_id, error=%err, "agent end db clear failed");
                    }

                    event_bus.send(AgentEvent::SessionAgentEnded {
                        session_id: session_id.clone(),
                        agent_name: name.clone(),
                        duration_ms,
                        // We don't have the exit code in the poll loop — the
                        // PTY reader thread has it on process wait(). Pass None
                        // here; the notifier will still emit the push with
                        // duration info and last-5 lines.
                        exit_code: None,
                    });
                    let _ = output_tx.send(WsOut::AgentEnded { agent_name: name });
                    prev_agent = None;
                }

                // (None, None) — covered by the first arm above.
                (None, None) => {}
            }
        }
    });
}

#[cfg(not(unix))]
pub fn spawn_agent_detector(
    _session_id: String,
    _db_path: PathBuf,
    _event_bus: Arc<EventBus>,
    _output_tx: broadcast::Sender<WsOut>,
) {
    // Agent detection not supported on non-Unix platforms (deferred to v2).
}

#[allow(clippy::too_many_arguments)]
/// Spawn a PTY session, register it in the in-memory `sessions` map, and
/// start the reader + idle watchdog threads. The reader thread auto-removes
/// the session from the map on PTY exit so the count gate in
/// `terminal_api::api_terminal_sessions_create` stays in sync with reality
/// (DB rows are the audit log; the map is the source of truth for "alive").
pub fn spawn_terminal_session(
    id: &str,
    owner_session_id: &str,
    cwd: Option<&str>,
    command: Vec<String>,
    cols: u16,
    rows: u16,
    db_path: PathBuf,
    sessions: Arc<DashMap<String, Arc<TerminalSession>>>,
    event_bus: Arc<EventBus>,
) -> anyhow::Result<Arc<TerminalSession>> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(&command[0]);
    for arg in command.iter().skip(1) {
        cmd.arg(arg);
    }
    if let Some(cwd) = cwd {
        cmd.cwd(cwd);
    }

    let child = pair.slave.spawn_command(cmd)?;
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    // Capture the raw fd BEFORE consuming master into the session struct so
    // the agent detector can call tcgetpgrp() on it. The MasterPty trait
    // exposes `as_raw_fd() -> Option<RawFd>` on Unix. Fall back to -1 if the
    // backend does not expose one (serial PTY etc.) — detector will return None.
    #[cfg(unix)]
    let master_fd: std::os::unix::io::RawFd = pair.master.as_raw_fd().unwrap_or(-1);

    let master = pair.master;

    let (output_tx, _) = broadcast::channel::<WsOut>(1024);
    let output_tx2 = output_tx.clone();

    let child = Arc::new(std::sync::Mutex::new(child));
    let child_for_wait = child.clone();
    let db_path_clone = db_path.clone();
    let id = id.to_string();
    let id_for_reader = id.clone();
    let sessions_for_reader = sessions.clone();

    let last_activity = Arc::new(std::sync::Mutex::new(Instant::now()));
    let last_activity2 = last_activity.clone();
    let last_activity3 = last_activity.clone();

    let buffer = Arc::new(std::sync::Mutex::new(RingBuffer::new(buffer_capacity_bytes())));
    let buffer_reader = buffer.clone();

    let last_seq = Arc::new(AtomicU64::new(0));
    let last_seq_reader = last_seq.clone();

    // Shared flag so the reader thread and watchdog agree on current broadcast state.
    // Reader flips true on output; watchdog flips false when idle threshold passes.
    let is_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_active_reader = is_active.clone();
    let is_active_watch = is_active.clone();
    let output_tx_watch = output_tx.clone();

    // Build the session and register it BEFORE the reader thread spawns so a
    // PTY that exits in microseconds (e.g. `false`) never races the map
    // insertion: the reader's self-removal would no-op and leave a phantom
    // entry that survives until the next /api/terminal/sessions list query.
    let session = Arc::new(TerminalSession {
        owner_session_id: owner_session_id.to_string(),
        writer: std::sync::Mutex::new(writer),
        output_tx: output_tx.clone(),
        child,
        master: std::sync::Mutex::new(master),
        last_activity,
        buffer,
        last_seq,
        #[cfg(unix)]
        master_fd,
    });
    sessions.insert(id.clone(), session.clone());

    // Start the 5s agent-detection polling loop (Unix only).
    #[cfg(unix)]
    spawn_agent_detector(
        id.clone(),
        master_fd,
        db_path.clone(),
        event_bus,
        output_tx.clone(),
    );
    #[cfg(not(unix))]
    {
        // Consume so the variable is not flagged unused.
        let _ = event_bus;
    }

    // Idle watchdog: emit State{idle} once output has been quiet for > ACTIVE_THRESHOLD.
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(150));
            // Stop when no subscribers AND no recent activity updates — the channel closes
            // when the session is dropped, which is our exit signal.
            if output_tx_watch.receiver_count() == 0 && Arc::strong_count(&last_activity3) <= 2 {
                break;
            }
            if !is_active_watch.load(Ordering::Relaxed) {
                continue;
            }
            let elapsed = last_activity3
                .lock()
                .map(|t| t.elapsed())
                .unwrap_or_default();
            if elapsed.as_millis() >= ACTIVE_THRESHOLD_MS {
                is_active_watch.store(false, Ordering::Relaxed);
                let _ = output_tx_watch.send(WsOut::State { state: "idle".into() });
            }
        }
    });

    std::thread::spawn(move || {
        use std::io::Read;
        use std::sync::atomic::Ordering;
        let mut r = reader;
        let mut buf = [0u8; 8192];
        let mut pending = Vec::<u8>::new();

        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    // Update activity timestamp on every output chunk.
                    if let Ok(mut t) = last_activity2.lock() {
                        *t = Instant::now();
                    }

                    pending.extend_from_slice(&buf[..n]);
                    let flush = |s: String| {
                        if s.is_empty() {
                            return;
                        }
                        let seq = buffer_reader
                            .lock()
                            .map(|mut b| b.append(s.clone()))
                            .unwrap_or(0);
                        if seq > 0 {
                            last_seq_reader.store(seq, Ordering::Relaxed);
                            let _ = output_tx2.send(WsOut::Chunk { seq, data: s });
                        }
                    };
                    match String::from_utf8(pending.clone()) {
                        Ok(s) => {
                            pending.clear();
                            flush(s);
                        }
                        Err(e) => {
                            let valid_up_to = e.utf8_error().valid_up_to();
                            if valid_up_to > 0 {
                                let s = String::from_utf8_lossy(&pending[..valid_up_to]).to_string();
                                flush(s);
                                pending = pending[valid_up_to..].to_vec();
                            }
                            if pending.len() > 256 * 1024 {
                                pending.clear();
                            }
                        }
                    }

                    // Broadcast state transition idle → active.
                    if !is_active_reader.swap(true, Ordering::Relaxed) {
                        let _ = output_tx2.send(WsOut::State { state: "active".into() });
                    }
                }
                Err(_) => break,
            }
        }

        // Recover from poisoned guard: if a previous holder panicked while
        // owning the lock, the data is still valid for our purposes (we only
        // need to call wait on the child). Without this, a single panic
        // upthread would propagate here and abort the reader thread before
        // it can update the DB / clear the in-memory map.
        let code = match child_for_wait
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .wait()
        {
            Ok(status) => Some(status.exit_code() as i64),
            Err(_) => None,
        };

        let now = now_ts();
        let _ = (|| -> anyhow::Result<()> {
            let conn = Connection::open(&db_path_clone)?;
            conn.execute(
                "UPDATE terminal_sessions SET status='exited', exit_code=?2, last_seen_at=?3 WHERE terminal_session_id=?1",
                params![id_for_reader, code, now],
            )?;
            Ok(())
        })();

        // Remove from the in-memory map so the per-user count gate in
        // `api_terminal_sessions_create` reflects reality immediately,
        // not after the next /api/terminal/sessions list query.
        sessions_for_reader.remove(&id_for_reader);

        let _ = output_tx2.send(WsOut::State { state: "exited".into() });
        let _ = output_tx2.send(WsOut::Exit { code });
    });

    Ok(session)
}
