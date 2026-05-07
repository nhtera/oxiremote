use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::db::now_ts;
use crate::AppState;

#[derive(Serialize)]
pub struct Workspace {
    pub id: i64,
    pub path: String,
    pub label: String,
    pub last_used_at: i64,
    pub pinned: bool,
}

/// Seed the workspaces table on first boot for a given host. Inserts HOME
/// only — the legacy "/" root-workspace seed was dropped in v0.1.26 (C3):
/// any approved tunnel device would otherwise gain read/write/delete on the
/// entire host filesystem. Existing DBs are NOT migrated; the dashboard
/// banner (see `has_root_workspace` + `/api/agent/hints`) prompts the
/// operator to remove it manually.
pub fn seed_defaults(conn: &Connection, host_id: &str) -> anyhow::Result<()> {
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workspaces WHERE host_id = ?1",
        params![host_id],
        |row| row.get(0),
    )?;
    if existing > 0 {
        return Ok(());
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    let now = now_ts();

    if let Some(home) = home.as_deref() {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO workspaces(host_id, path, label, last_used_at, pinned)
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![host_id, home, "Home", now],
        );
    }
    Ok(())
}

/// Detect a pre-existing `/` (or Windows `C:\`) workspace row from a legacy
/// install. Drives the dashboard `root-workspace-present` hint so the
/// operator is prompted to delete it. Existing rows are intentionally NOT
/// auto-removed (would invalidate device sessions referencing them).
pub fn has_root_workspace(conn: &Connection, host_id: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workspaces
         WHERE host_id = ?1 AND (path = '/' OR path = 'C:\\')",
        params![host_id],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// Resolve a registered workspace path by id, scoped to the given host.
pub fn lookup_path(db_path: &Path, host_id: &str, id: i64) -> Result<PathBuf, &'static str> {
    let conn = Connection::open(db_path).map_err(|_| "db open failed")?;
    let path: String = conn
        .query_row(
            "SELECT path FROM workspaces WHERE id = ?1 AND host_id = ?2",
            params![id, host_id],
            |row| row.get(0),
        )
        .map_err(|_| "workspace not found")?;
    let buf = PathBuf::from(path);
    if !buf.is_absolute() {
        return Err("workspace path not absolute");
    }
    Ok(buf)
}

fn list_for_host(db_path: &Path, host_id: &str) -> anyhow::Result<Vec<Workspace>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, path, label, last_used_at, pinned
         FROM workspaces WHERE host_id = ?1
         ORDER BY pinned DESC, last_used_at DESC",
    )?;
    let rows = stmt.query_map(params![host_id], |row| {
        Ok(Workspace {
            id: row.get(0)?,
            path: row.get(1)?,
            label: row.get(2)?,
            last_used_at: row.get(3)?,
            pinned: row.get::<_, i64>(4)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub async fn api_workspaces_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match list_for_host(&state.db_path, &state.host_info.host_id) {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(error = %e, "workspaces list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CreateWorkspace {
    path: String,
    #[serde(default)]
    label: Option<String>,
}

pub async fn api_workspaces_create(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateWorkspace>,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let trimmed = req.path.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let path_buf = std::path::PathBuf::from(trimmed);
    if !path_buf.is_absolute() {
        return (StatusCode::BAD_REQUEST, "path must be absolute").into_response();
    }
    let canon = match path_buf.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::BAD_REQUEST, "path not found").into_response(),
    };
    if !canon.is_dir() {
        return (StatusCode::BAD_REQUEST, "not a directory").into_response();
    }

    let stored = canon.to_string_lossy().to_string();
    let label = req
        .label
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(80).collect::<String>())
        .unwrap_or_else(|| {
            canon
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| stored.clone())
        });

    let conn = match Connection::open(&state.db_path) {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let now = now_ts();
    let res = conn.execute(
        "INSERT INTO workspaces(host_id, path, label, last_used_at, pinned)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(host_id, path) DO UPDATE SET
           label = excluded.label,
           last_used_at = excluded.last_used_at",
        params![state.host_info.host_id, stored, label, now],
    );
    if let Err(e) = res {
        warn!(error = %e, "workspace upsert failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match list_for_host(&state.db_path, &state.host_info.host_id) {
        Ok(rows) => (StatusCode::CREATED, Json(rows)).into_response(),
        Err(_) => StatusCode::OK.into_response(),
    }
}

pub async fn api_workspaces_delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let conn = match Connection::open(&state.db_path) {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let affected = conn
        .execute(
            "DELETE FROM workspaces WHERE id = ?1 AND host_id = ?2 AND pinned = 0",
            params![id, state.host_info.host_id],
        )
        .unwrap_or(0);
    if affected == 0 {
        return (StatusCode::BAD_REQUEST, "cannot remove (pinned or not found)").into_response();
    }
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
pub struct ValidateWorkspace {
    path: String,
}

#[derive(Serialize)]
pub struct ValidateWorkspaceResponse {
    exists: bool,
    kind: &'static str, // "dir" | "file" | "missing"
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// POST /api/workspace/validate — cheap pre-flight before workspace creation.
/// Auth-required (loopback or Bearer + CSRF). Returns kind so the SPA can
/// show inline errors instead of waiting for the create round-trip.
pub async fn api_workspace_validate(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Json(req): Json<ValidateWorkspace>,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let trimmed = req.path.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Json(ValidateWorkspaceResponse {
            exists: false,
            kind: "missing",
            message: Some("Path is empty.".to_string()),
        })
        .into_response();
    }
    let path_buf = PathBuf::from(trimmed);
    if !path_buf.is_absolute() {
        return Json(ValidateWorkspaceResponse {
            exists: false,
            kind: "missing",
            message: Some("Path must be absolute.".to_string()),
        })
        .into_response();
    }
    let canon = match path_buf.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Json(ValidateWorkspaceResponse {
                exists: false,
                kind: "missing",
                message: Some("Path does not exist.".to_string()),
            })
            .into_response();
        }
    };
    if canon.is_dir() {
        return Json(ValidateWorkspaceResponse {
            exists: true,
            kind: "dir",
            message: None,
        })
        .into_response();
    }
    let kind = if canon.is_file() { "file" } else { "missing" };
    Json(ValidateWorkspaceResponse {
        exists: true,
        kind,
        message: Some("Path is not a directory.".to_string()),
    })
    .into_response()
}

pub async fn api_workspaces_touch(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let conn = match Connection::open(&state.db_path) {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let _ = conn.execute(
        "UPDATE workspaces SET last_used_at = ?1 WHERE id = ?2 AND host_id = ?3",
        params![now_ts(), id, state.host_info.host_id],
    );
    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    fn fresh_db(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "oxi-ws-{}-{}-{name}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&p);
        init_db(&p).unwrap();
        p
    }

    #[test]
    fn fresh_install_does_not_seed_root_workspace() {
        let p = fresh_db("no-root");
        let conn = Connection::open(&p).unwrap();
        seed_defaults(&conn, "host-a").unwrap();

        let count_root: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE host_id = 'host-a' AND path = '/'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_root, 0, "v0.1.26+ must not seed `/` (C3)");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn existing_root_workspace_is_preserved_on_reseed() {
        let p = fresh_db("existing-root");
        let conn = Connection::open(&p).unwrap();
        // Simulate a legacy install with the `/` row.
        conn.execute(
            "INSERT INTO workspaces(host_id, path, label, last_used_at, pinned)
             VALUES (?1, '/', 'Root (/)', ?2, 1)",
            params!["host-a", now_ts()],
        )
        .unwrap();

        // Re-seeding (idempotent boot) must not error and must not delete
        // the existing row.
        seed_defaults(&conn, "host-a").unwrap();

        let still_there: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE host_id = 'host-a' AND path = '/'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 1, "existing `/` row must be preserved");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn has_root_workspace_detects_unix_root() {
        let p = fresh_db("detect-root");
        let conn = Connection::open(&p).unwrap();

        assert!(!has_root_workspace(&conn, "host-a").unwrap());

        conn.execute(
            "INSERT INTO workspaces(host_id, path, label, last_used_at, pinned)
             VALUES ('host-a', '/', 'Root', ?1, 1)",
            params![now_ts()],
        )
        .unwrap();
        assert!(has_root_workspace(&conn, "host-a").unwrap());
        // Other hosts not affected.
        assert!(!has_root_workspace(&conn, "host-b").unwrap());

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn has_root_workspace_detects_windows_root() {
        let p = fresh_db("detect-root-win");
        let conn = Connection::open(&p).unwrap();

        conn.execute(
            "INSERT INTO workspaces(host_id, path, label, last_used_at, pinned)
             VALUES ('host-a', 'C:\\', 'Root', ?1, 1)",
            params![now_ts()],
        )
        .unwrap();
        assert!(has_root_workspace(&conn, "host-a").unwrap());

        let _ = std::fs::remove_file(&p);
    }
}
