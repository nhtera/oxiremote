use std::collections::HashMap;
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

/// Per-directory git status overlay used by the file browser. Returns a map
/// of entry-name (relative to `dir`) → single-letter status char so the SPA
/// can paint a colored stripe in the file tree without parsing porcelain.
///
/// Status precedence: working-tree state wins over index, so a file modified
/// after staging shows 'M' (the most actionable signal for the user). Caps
/// the parsed line count at 1000 so a worst-case repo can't blow latency.
pub async fn status_for_dir(repo: &Path, dir: &Path) -> HashMap<String, char> {
    let mut map = HashMap::new();
    let dir_rel = dir
        .strip_prefix(repo)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");

    let mut cmd = Command::new("git");
    cmd.arg("status").arg("--porcelain=v1").arg("--untracked-files=all").arg("--");
    if dir_rel.is_empty() {
        cmd.arg(".");
    } else {
        cmd.arg(&dir_rel);
    }
    cmd.current_dir(repo);

    let Ok(output) = cmd.output().await else { return map };
    if !output.status.success() {
        return map;
    }

    for (i, line) in String::from_utf8_lossy(&output.stdout).lines().enumerate() {
        if i >= 1000 {
            break;
        }
        if line.len() < 4 {
            continue;
        }
        let mut chars = line.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        // Working tree (`Y`) takes precedence over index (`X`) because that's
        // what the operator most cares about when scanning the tree.
        let c = match (x, y) {
            ('?', '?') => '?',
            (_, 'M') | ('M', _) => 'M',
            (_, 'A') | ('A', _) => 'A',
            (_, 'D') | ('D', _) => 'D',
            (_, 'R') | ('R', _) => 'R',
            _ => continue,
        };
        let path_part = &line[3..];
        // Trim renamed-from segment ("oldpath -> newpath"); the new name is
        // what shows in the listing. `Split<&str>` isn't DoubleEndedIterator
        // so we use `rfind` to slice past the last separator instead.
        let path_part = match path_part.rfind(" -> ") {
            Some(idx) => &path_part[idx + 4..],
            None => path_part,
        };
        // Strip the dir prefix, then take the first segment so subdir entries
        // bubble up as their containing-dir name in the listing.
        let stripped = if dir_rel.is_empty() {
            path_part
        } else {
            let prefix = format!("{dir_rel}/");
            path_part.strip_prefix(&prefix).unwrap_or(path_part)
        };
        let entry_name = stripped.split('/').next().unwrap_or(stripped).to_string();
        // Don't downgrade a stronger status: M > A > ? > D for display purposes.
        map.entry(entry_name).or_insert(c);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn temp_repo(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "oxi-git-test-{label}-{}-{}",
            std::process::id(),
            crate::db::now_ts()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        let init_ok = StdCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&p)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(init_ok, "git init failed — is git on PATH?");
        // Set up identity so commit doesn't fail in CI.
        let _ = StdCommand::new("git").args(["config", "user.email", "t@t"]).current_dir(&p).status();
        let _ = StdCommand::new("git").args(["config", "user.name", "test"]).current_dir(&p).status();
        let _ = StdCommand::new("git").args(["config", "commit.gpgsign", "false"]).current_dir(&p).status();
        p
    }

    #[tokio::test]
    async fn status_for_dir_parses_modified_added_deleted_untracked() {
        let repo = temp_repo("status");
        std::fs::write(repo.join("kept.txt"), b"keep").unwrap();
        std::fs::write(repo.join("doomed.txt"), b"bye").unwrap();
        let _ = StdCommand::new("git").args(["add", "-A"]).current_dir(&repo).status();
        let _ = StdCommand::new("git").args(["commit", "-m", "init", "--quiet"]).current_dir(&repo).status();

        // Modify a tracked file, delete a tracked file, add a new + untracked.
        std::fs::write(repo.join("kept.txt"), b"changed").unwrap();
        std::fs::remove_file(repo.join("doomed.txt")).unwrap();
        std::fs::write(repo.join("brand-new.txt"), b"x").unwrap();
        let _ = StdCommand::new("git").args(["add", "brand-new.txt"]).current_dir(&repo).status();
        std::fs::write(repo.join("scratch.tmp"), b"u").unwrap();

        let map = status_for_dir(&repo, &repo).await;
        assert_eq!(map.get("kept.txt"), Some(&'M'));
        assert_eq!(map.get("doomed.txt"), Some(&'D'));
        assert_eq!(map.get("brand-new.txt"), Some(&'A'));
        assert_eq!(map.get("scratch.tmp"), Some(&'?'));

        let _ = std::fs::remove_dir_all(&repo);
    }
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
