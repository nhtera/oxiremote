use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::db::now_ts;

pub const MAX_TERMINAL_SESSIONS_PER_USER: usize = 10;
pub const WS_MAX_TEXT_BYTES: usize = 64 * 1024;

/// Threshold for considering a session "active" (recently produced output).
const ACTIVE_THRESHOLD_MS: u128 = 250;

pub struct TerminalSession {
    pub owner_session_id: String,
    pub writer: std::sync::Mutex<Box<dyn std::io::Write + Send>>,
    pub output_tx: broadcast::Sender<WsOut>,
    pub child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    pub master: std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    /// Tracks the last time output was received from the PTY.
    pub last_activity: Arc<std::sync::Mutex<Instant>>,
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
}

#[derive(Deserialize)]
pub struct RenameTerminalRequest {
    pub name: String,
}

#[derive(Clone, Serialize)]
#[serde(tag = "t")]
pub enum WsOut {
    #[serde(rename = "output")]
    Output { data: String },
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
    if let Some(cmd) = command {
        if !cmd.is_empty() {
            return cmd.to_vec();
        }
    }
    if std::env::consts::OS == "macos" {
        vec!["/bin/zsh".into(), "-l".into()]
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        vec![shell, "-l".into()]
    }
}

pub fn spawn_terminal_session(
    id: &str,
    owner_session_id: &str,
    cwd: Option<&str>,
    command: Vec<String>,
    cols: u16,
    rows: u16,
    db_path: PathBuf,
) -> anyhow::Result<TerminalSession> {
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

    let last_activity = Arc::new(std::sync::Mutex::new(Instant::now()));
    let last_activity2 = last_activity.clone();
    let last_activity3 = last_activity.clone();

    // Shared flag so the reader thread and watchdog agree on current broadcast state.
    // Reader flips true on output; watchdog flips false when idle threshold passes.
    let is_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_active_reader = is_active.clone();
    let is_active_watch = is_active.clone();
    let output_tx_watch = output_tx.clone();

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
                    match String::from_utf8(pending.clone()) {
                        Ok(s) => {
                            pending.clear();
                            let _ = output_tx2.send(WsOut::Output { data: s });
                        }
                        Err(e) => {
                            let valid_up_to = e.utf8_error().valid_up_to();
                            if valid_up_to > 0 {
                                let s = String::from_utf8_lossy(&pending[..valid_up_to]).to_string();
                                let _ = output_tx2.send(WsOut::Output { data: s });
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

        let code = match child_for_wait.lock().unwrap().wait() {
            Ok(status) => Some(status.exit_code() as i64),
            Err(_) => None,
        };

        let now = now_ts();
        let _ = (|| -> anyhow::Result<()> {
            let conn = Connection::open(&db_path)?;
            conn.execute(
                "UPDATE terminal_sessions SET status='exited', exit_code=?2, last_seen_at=?3 WHERE terminal_session_id=?1",
                params![id.to_string(), code, now],
            )?;
            Ok(())
        })();

        let _ = output_tx2.send(WsOut::State { state: "exited".into() });
        let _ = output_tx2.send(WsOut::Exit { code });
    });

    Ok(TerminalSession {
        owner_session_id: owner_session_id.to_string(),
        writer: std::sync::Mutex::new(writer),
        output_tx,
        child,
        master: std::sync::Mutex::new(master),
        last_activity,
    })
}
