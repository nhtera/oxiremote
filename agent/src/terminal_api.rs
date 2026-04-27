use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use portable_pty::PtySize;
use rusqlite::{params, Connection, OptionalExtension};
use tracing::warn;
use uuid::Uuid;

use crate::auth::require_active_auth;
use crate::db::now_ts;
use crate::terminal_pty::{
    build_default_command, spawn_terminal_session, CreateTerminalSessionRequest,
    CreateTerminalSessionResponse, RenameTerminalRequest, ResizeTerminalRequest, TerminalSessionMeta,
    WsOut, MAX_TERMINAL_SESSIONS_PER_USER,
};
use crate::AppState;

pub async fn api_terminal_sessions_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let host_id = state.host_info.host_id.clone();

    let res: anyhow::Result<Vec<TerminalSessionMeta>> = (|| {
        let conn = Connection::open(&state.db_path)?;
        // Hide administratively closed sessions ('closed' = user clicked close,
        // 'dead' = WS evicted). 'exited' (process died naturally) stays visible
        // so the user sees the red dot before deciding whether to close the tab.
        let mut stmt = conn.prepare(
            "SELECT terminal_session_id, name, created_at, last_seen_at, cols, rows, status, exit_code \
             FROM terminal_sessions \
             WHERE owner_session_id=?1 AND status NOT IN ('closed','dead') \
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![owner_session_id], |row| {
            let id: String = row.get(0)?;
            let status: String = row.get(6)?;
            Ok((id, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, status, row.get(7)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, created_at, last_seen_at, cols, rows_v, status, exit_code): (
                String, Option<String>, i64, i64, i64, i64, String, Option<i64>,
            ) = r?;
            let sess = state.terminal_sessions.get(&id);
            // Compute live state from in-memory session if available, else derive from DB status.
            let state_str = sess
                .as_ref()
                .map(|s| s.state(&status).to_string())
                .unwrap_or_else(|| {
                    if status == "running" { "idle".into() } else { "exited".into() }
                });
            let (last_seq, buffer_bytes, attached) = match sess.as_ref() {
                Some(s) => {
                    let seq = s.last_seq.load(Ordering::Relaxed);
                    let bytes = s
                        .buffer
                        .lock()
                        .map(|b| b.byte_len() as u64)
                        .unwrap_or(0);
                    let atk = s.output_tx.receiver_count() > 0;
                    (seq, bytes, atk)
                }
                None => (0, 0, false),
            };
            out.push(TerminalSessionMeta {
                id,
                name,
                host_id: host_id.clone(),
                state: state_str,
                created_at,
                last_seen_at,
                cols,
                rows: rows_v,
                status,
                exit_code,
                last_seq,
                buffer_bytes,
                attached,
            });
        }
        Ok(out)
    })();

    match res {
        Ok(list) => (StatusCode::OK, Json(list)).into_response(),
        Err(err) => {
            warn!(error=%err, "list terminal sessions failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn api_terminal_sessions_create(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<CreateTerminalSessionRequest>,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let existing = count_owned_sessions(
        state.terminal_sessions.iter().map(|s| s.value().owner_session_id.clone()),
        &owner_session_id,
    );

    if existing >= MAX_TERMINAL_SESSIONS_PER_USER {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    let id = Uuid::new_v4().to_string();
    let now = now_ts();
    let cols_i64 = req.cols as i64;
    let rows_i64 = req.rows as i64;
    let status_str = "running".to_string();

    let insert_res: anyhow::Result<()> = (|| {
        let conn = Connection::open(&state.db_path)?;
        conn.execute(
            "INSERT INTO terminal_sessions(terminal_session_id, owner_session_id, name, cwd, command, created_at, last_seen_at, cols, rows, status, exit_code) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, NULL)",
            params![
                id,
                owner_session_id,
                req.name,
                req.cwd,
                req.command.as_ref().map(|v| serde_json::to_string(v).unwrap()),
                now,
                cols_i64,
                rows_i64,
                status_str,
            ],
        )?;
        Ok(())
    })();

    if let Err(err) = insert_res {
        warn!(error=%err, "insert terminal session failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let shell_cmd = build_default_command(req.command.as_deref());
    let cwd = req.cwd.clone();

    // spawn_terminal_session inserts into terminal_sessions itself so the
    // PTY's reader thread can self-remove on exit without a TOCTOU window.
    match spawn_terminal_session(
        &id,
        &owner_session_id,
        cwd.as_deref(),
        shell_cmd,
        req.cols,
        req.rows,
        state.db_path.clone(),
        state.terminal_sessions.clone(),
    ) {
        Ok(_sess) => {
            (StatusCode::OK, Json(CreateTerminalSessionResponse { id })).into_response()
        }
        Err(err) => {
            warn!(error=%err, "spawn terminal session failed");
            let _ = (|| -> anyhow::Result<()> {
                let conn = Connection::open(&state.db_path)?;
                let now = now_ts();
                conn.execute(
                    "UPDATE terminal_sessions SET status='dead', last_seen_at=?2 WHERE terminal_session_id=?1",
                    params![id, now],
                )?;
                Ok(())
            })();
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn api_terminal_session_resize(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ResizeTerminalRequest>,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let owned: anyhow::Result<bool> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT 1 FROM terminal_sessions WHERE terminal_session_id=?1 AND owner_session_id=?2",
        )?;
        Ok(stmt.exists(params![id, owner_session_id])?)
    })();

    let owned = match owned {
        Ok(v) => v,
        Err(err) => {
            warn!(error=%err, "resize terminal session ownership check failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !owned {
        return StatusCode::FORBIDDEN.into_response();
    }

    let Some(sess) = state.terminal_sessions.get(&id) else {
        let _ = (|| -> anyhow::Result<()> {
            let conn = Connection::open(&state.db_path)?;
            let now = now_ts();
            conn.execute(
                "UPDATE terminal_sessions SET status='dead', last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?3",
                params![id, now, owner_session_id],
            )?;
            Ok(())
        })();

        return (
            StatusCode::GONE,
            Json(serde_json::json!({ "error": "session not running" })),
        )
            .into_response();
    };

    if let Ok(m) = sess.master.lock() {
        let _ = m.resize(PtySize {
            rows: req.rows,
            cols: req.cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    let res: anyhow::Result<()> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        conn.execute(
            "UPDATE terminal_sessions SET cols=?3, rows=?4, last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?5",
            params![id, now, req.cols as i64, req.rows as i64, owner_session_id],
        )?;
        Ok(())
    })();

    if let Err(err) = res {
        warn!(error=%err, "resize terminal session update failed");
    }

    StatusCode::OK.into_response()
}

pub async fn api_terminal_session_close(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let session_status: anyhow::Result<Option<String>> = (|| {
        let conn = Connection::open(&state.db_path)?;
        conn.query_row(
            "SELECT status FROM terminal_sessions WHERE terminal_session_id=?1 AND owner_session_id=?2",
            params![id, owner_session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    })();

    let status = match session_status {
        Ok(v) => v,
        Err(err) => {
            warn!(error=%err, "close terminal session ownership check failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let Some(status) = status else {
        return StatusCode::FORBIDDEN.into_response();
    };

    if status == "closed" || status == "dead" {
        // Already gone from the user's perspective — nothing to update.
        return StatusCode::OK.into_response();
    }
    if status == "exited" {
        // PTY died naturally; user is now actively dismissing the tab. Flip
        // the row to 'closed' so the list query filters it out and the tab
        // disappears on the next refresh.
        let _ = (|| -> anyhow::Result<()> {
            let conn = Connection::open(&state.db_path)?;
            let now = now_ts();
            conn.execute(
                "UPDATE terminal_sessions SET status='closed', last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?3",
                params![id, now, owner_session_id],
            )?;
            Ok(())
        })();
        return StatusCode::OK.into_response();
    }

    let Some((_, sess)) = state.terminal_sessions.remove(&id) else {
        let _ = (|| -> anyhow::Result<()> {
            let conn = Connection::open(&state.db_path)?;
            let now = now_ts();
            conn.execute(
                "UPDATE terminal_sessions SET status='dead', last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?3",
                params![id, now, owner_session_id],
            )?;
            Ok(())
        })();

        return (
            StatusCode::GONE,
            Json(serde_json::json!({ "error": "session not running" })),
        )
            .into_response();
    };

    if let Ok(mut child) = sess.child.lock() {
        let _ = child.kill();
    }

    let res: anyhow::Result<()> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        conn.execute(
            "UPDATE terminal_sessions SET status='closed', last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?3",
            params![id, now, owner_session_id],
        )?;
        Ok(())
    })();

    if let Err(err) = res {
        warn!(error=%err, "close terminal session update failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::OK.into_response()
}

pub async fn api_terminal_session_rename(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RenameTerminalRequest>,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    // Validate name: non-empty, max 64 chars.
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "name must be 1–64 characters" })),
        )
            .into_response();
    }

    let updated: anyhow::Result<bool> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let n = conn.execute(
            "UPDATE terminal_sessions SET name=?3 WHERE terminal_session_id=?1 AND owner_session_id=?2",
            params![id, owner_session_id, name],
        )?;
        Ok(n > 0)
    })();

    match updated {
        Ok(true) => {
            // Broadcast rename event to all WS subscribers of this session.
            if let Some(sess) = state.terminal_sessions.get(&id) {
                let _ = sess.output_tx.send(WsOut::Renamed { name });
            }
            StatusCode::OK.into_response()
        }
        Ok(false) => StatusCode::FORBIDDEN.into_response(),
        Err(err) => {
            warn!(error=%err, "rename terminal session failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// On agent boot, any DB row still marked `running` must be orphaned — the PTY
/// process died with the previous agent. Mark them `dead` so the list endpoint
/// Count live PTY sessions owned by `target`. Caller projects an iterator of
/// owner ids from the in-memory `terminal_sessions` DashMap — that map is the
/// source of truth (a PTY that died after spawn but before the DB row was
/// marked `dead` would otherwise leave the DB count permanently over the
/// cap). The reader thread auto-removes from this map on PTY exit (see
/// `spawn_terminal_session`), so this count tracks reality.
pub fn count_owned_sessions<S, I>(owner_ids: I, target: &str) -> usize
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    owner_ids.into_iter().filter(|o| o.as_ref() == target).count()
}

/// and WS handler stay consistent without waiting for a subscriber to notice.
pub fn reconcile_orphan_sessions(db_path: &Path) -> anyhow::Result<usize> {
    let conn = Connection::open(db_path)?;
    let now = now_ts();
    let n = conn.execute(
        "UPDATE terminal_sessions SET status='dead', last_seen_at=?1 WHERE status='running'",
        params![now],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin: per-user cap gate counts from the iterator the caller supplies.
    /// Production code wires that iterator to the in-memory DashMap, NOT a DB
    /// query — a stale DB row from a crashed PTY would otherwise wedge a user
    /// at the cap until manual cleanup.
    #[test]
    fn session_count_uses_inmemory_map() {
        let owners = ["alice", "bob", "alice", "carol", "alice"];
        assert_eq!(count_owned_sessions(owners.iter().copied(), "alice"), 3);
        assert_eq!(count_owned_sessions(owners.iter().copied(), "bob"), 1);
        assert_eq!(count_owned_sessions(owners.iter().copied(), "dave"), 0);
        // Empty input — fresh process, no live PTYs yet.
        let empty: [&str; 0] = [];
        assert_eq!(count_owned_sessions(empty.iter().copied(), "alice"), 0);
    }
}
