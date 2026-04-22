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
    CreateTerminalSessionResponse, ResizeTerminalRequest, TerminalSessionMeta,
    MAX_TERMINAL_SESSIONS_PER_USER,
};
use crate::AppState;

pub async fn api_terminal_sessions_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let res: anyhow::Result<Vec<TerminalSessionMeta>> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT terminal_session_id, name, created_at, last_seen_at, cols, rows, status, exit_code \
             FROM terminal_sessions WHERE owner_session_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![owner_session_id], |row| {
            Ok(TerminalSessionMeta {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                last_seen_at: row.get(3)?,
                cols: row.get(4)?,
                rows: row.get(5)?,
                status: row.get(6)?,
                exit_code: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
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

    let existing: anyhow::Result<i64> = (|| {
        let conn = Connection::open(&state.db_path)?;
        conn.query_row(
            "SELECT COUNT(*) FROM terminal_sessions WHERE owner_session_id=?1 AND status='running'",
            params![owner_session_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    })();

    let existing = match existing {
        Ok(count) => count as usize,
        Err(err) => {
            warn!(error=%err, "count terminal sessions failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

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

    match spawn_terminal_session(
        &id,
        &owner_session_id,
        cwd.as_deref(),
        shell_cmd,
        req.cols,
        req.rows,
        state.db_path.clone(),
    ) {
        Ok(sess) => {
            state
                .terminal_sessions
                .insert(id.clone(), Arc::new(sess));
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

    if status == "closed" || status == "exited" || status == "dead" {
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
