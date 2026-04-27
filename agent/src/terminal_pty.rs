use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::db::now_ts;
use crate::terminal_buffer::{RingBuffer, Snapshot};

pub const MAX_TERMINAL_SESSIONS_PER_USER: usize = 10;
pub const WS_MAX_TEXT_BYTES: usize = 64 * 1024;

/// Threshold for considering a session "active" (recently produced output).
const ACTIVE_THRESHOLD_MS: u128 = 250;

/// Default ring buffer capacity (bytes). Override via `OXI_PTY_BUFFER_BYTES`.
const DEFAULT_BUFFER_BYTES: usize = 2 * 1024 * 1024;

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
    if std::env::consts::OS == "macos" {
        vec!["/bin/zsh".into(), "-l".into()]
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        vec![shell, "-l".into()]
    }
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
    let master = pair.master;

    let (output_tx, _) = broadcast::channel::<WsOut>(1024);
    let output_tx2 = output_tx.clone();

    let child = Arc::new(std::sync::Mutex::new(child));
    let child_for_wait = child.clone();
    let db_path = db_path.clone();
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
    });
    sessions.insert(id, session.clone());

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
            let conn = Connection::open(&db_path)?;
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
