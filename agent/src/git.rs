use std::path::Path;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::warn;

use crate::auth::require_active_auth;
use crate::AppState;

fn workspace_root(state: &AppState) -> &Path {
    &state.workspace_root
}

fn sanitize_paths(paths: &[String], root: &Path) -> Result<Vec<String>, &'static str> {
    for p in paths {
        if p.contains("..") || p.starts_with('/') || p.starts_with('\\') {
            return Err("invalid path");
        }
        let resolved = root.join(p);
        if !resolved.starts_with(root) {
            return Err("path escapes workspace");
        }
    }
    Ok(paths.to_vec())
}

async fn run_git(root: &Path, args: &[&str]) -> Result<String, (StatusCode, String)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|e| {
            warn!(error=%e, "git exec failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "git exec failed".into())
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        warn!(args=?args, stderr=%stderr, "git command failed");
        return Err((StatusCode::BAD_REQUEST, stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Serialize)]
struct GitStatusEntry {
    path: String,
    index: String,
    working: String,
}

pub async fn api_git_status(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    let Some(_) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = workspace_root(&state);
    let out = match run_git(root, &["status", "--porcelain=v1", "-uall"]).await {
        Ok(v) => v,
        Err((code, msg)) => return (code, msg).into_response(),
    };

    let entries: Vec<GitStatusEntry> = out
        .lines()
        .filter(|l| l.len() >= 3)
        .map(|line| {
            let index = line.chars().next().unwrap_or(' ').to_string();
            let working = line.chars().nth(1).unwrap_or(' ').to_string();
            let path = line[3..].to_string();
            GitStatusEntry { path, index, working }
        })
        .collect();

    (StatusCode::OK, Json(entries)).into_response()
}

#[derive(Deserialize)]
pub struct DiffQuery {
    #[serde(default)]
    staged: Option<u8>,
    path: Option<String>,
}

pub async fn api_git_diff(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<DiffQuery>,
) -> impl IntoResponse {
    let Some(_) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = workspace_root(&state);
    let mut args = vec!["diff"];
    if q.staged.unwrap_or(0) == 1 {
        args.push("--cached");
    }

    let path_str;
    if let Some(ref p) = q.path {
        if sanitize_paths(std::slice::from_ref(p), root).is_err() {
            return (StatusCode::BAD_REQUEST, "invalid path").into_response();
        }
        args.push("--");
        path_str = p.clone();
        args.push(&path_str);
    }

    let out = match run_git(root, &args).await {
        Ok(v) => v,
        Err((code, msg)) => return (code, msg).into_response(),
    };

    (StatusCode::OK, out).into_response()
}

#[derive(Deserialize)]
pub struct StageRequest {
    paths: Vec<String>,
}

pub async fn api_git_stage(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<StageRequest>,
) -> impl IntoResponse {
    let Some(_) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = workspace_root(&state);
    if sanitize_paths(&req.paths, root).is_err() {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    let mut args: Vec<&str> = vec!["add", "--"];
    let paths: Vec<&str> = req.paths.iter().map(|s| s.as_str()).collect();
    args.extend(paths);

    match run_git(root, &args).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

pub async fn api_git_unstage(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<StageRequest>,
) -> impl IntoResponse {
    let Some(_) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let root = workspace_root(&state);
    if sanitize_paths(&req.paths, root).is_err() {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    let mut args: Vec<&str> = vec!["reset", "HEAD", "--"];
    let paths: Vec<&str> = req.paths.iter().map(|s| s.as_str()).collect();
    args.extend(paths);

    match run_git(root, &args).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CommitRequest {
    message: String,
}

pub async fn api_git_commit(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<CommitRequest>,
) -> impl IntoResponse {
    let Some(_) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if req.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty commit message").into_response();
    }

    let root = workspace_root(&state);
    let msg = req.message.clone();
    match run_git(root, &["commit", "-m", &msg]).await {
        Ok(out) => (StatusCode::OK, out).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}
