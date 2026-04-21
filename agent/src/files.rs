use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::warn;

use crate::auth::require_auth;
use crate::AppState;

const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

fn resolve_safe_path(root: &Path, rel: &str) -> Result<std::path::PathBuf, &'static str> {
    if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
        return Err("invalid path");
    }
    let resolved = root.join(rel).canonicalize().map_err(|_| "path not found")?;
    let canon_root = root.canonicalize().map_err(|_| "workspace root invalid")?;
    if !resolved.starts_with(&canon_root) {
        return Err("path escapes workspace");
    }
    Ok(resolved)
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: i64,
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    path: Option<String>,
}

pub async fn api_files_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let Some(_) = require_auth(&state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = &state.workspace_root;
    let dir = match q.path.as_deref().filter(|p| !p.is_empty()) {
        Some(rel) => match resolve_safe_path(root, rel) {
            Ok(p) => p,
            Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
        },
        None => root.canonicalize().unwrap_or_else(|_| root.clone()),
    };

    if !dir.is_dir() {
        return (StatusCode::BAD_REQUEST, "not a directory").into_response();
    }

    let mut entries = Vec::new();
    let mut read_dir = match fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => {
            warn!(error=%e, "read_dir failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        entries.push(FileEntry {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified,
        });
    }

    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    (StatusCode::OK, Json(entries)).into_response()
}

#[derive(Deserialize)]
pub struct ReadQuery {
    path: String,
}

pub async fn api_files_read(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<ReadQuery>,
) -> impl IntoResponse {
    let Some(_) = require_auth(&state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = &state.workspace_root;
    let file_path = match resolve_safe_path(root, &q.path) {
        Ok(p) => p,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };

    if !file_path.is_file() {
        return (StatusCode::BAD_REQUEST, "not a file").into_response();
    }

    let meta = match fs::metadata(&file_path).await {
        Ok(m) => m,
        Err(e) => {
            warn!(error=%e, "metadata failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if meta.len() > MAX_FILE_SIZE {
        return (StatusCode::BAD_REQUEST, "file too large (max 1MB)").into_response();
    }

    match fs::read_to_string(&file_path).await {
        Ok(content) => (StatusCode::OK, content).into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "binary or unreadable file").into_response(),
    }
}

#[derive(Deserialize)]
pub struct WriteRequest {
    path: String,
    content: String,
}

pub async fn api_files_write(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<WriteRequest>,
) -> impl IntoResponse {
    let Some(_) = require_auth(&state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if req.content.len() as u64 > MAX_FILE_SIZE {
        return (StatusCode::BAD_REQUEST, "content too large (max 1MB)").into_response();
    }

    let root = &state.workspace_root;
    // For writes, the file may not exist yet — validate path components manually
    if req.path.contains("..") || req.path.starts_with('/') || req.path.starts_with('\\') {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    let file_path = root.join(&req.path);
    let canon_root = match root.canonicalize() {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Check parent exists and resolves within root
    if let Some(parent) = file_path.parent() {
        if let Ok(canon_parent) = parent.canonicalize() {
            if !canon_parent.starts_with(&canon_root) {
                return (StatusCode::BAD_REQUEST, "path escapes workspace").into_response();
            }
        } else {
            return (StatusCode::BAD_REQUEST, "parent directory does not exist").into_response();
        }
    }

    if let Err(e) = fs::write(&file_path, &req.content).await {
        warn!(error=%e, "write file failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
    }

    StatusCode::OK.into_response()
}
