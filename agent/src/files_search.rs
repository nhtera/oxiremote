// Filename search across a workspace. Backed by the `ignore` crate so
// `.gitignore` and `.git/` are skipped without us reimplementing the rules.
// Caps result count + walk depth so a misclick on a giant tree (`/home`) can't
// pin a server thread for seconds.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};

use crate::auth::require_active_auth;
use crate::files::active_root;
use crate::AppState;

const MAX_HITS: usize = 200;
const MAX_DEPTH: usize = 20;
const MIN_QUERY_LEN: usize = 2;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize)]
pub struct SearchHit {
    /// Workspace-relative path with forward slashes (cross-platform UI).
    pub path: String,
    /// Just the filename — bolded in the dropdown so the operator can scan.
    pub name: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    #[serde(default)]
    ws_id: Option<i64>,
}

/// Walk the workspace and return up to `MAX_HITS` filename matches for `q`.
/// Case-insensitive substring match. Skips gitignored paths and hidden dirs.
pub fn search(workspace: &Path, query: &str) -> Vec<SearchHit> {
    let q = query.trim().to_lowercase();
    if q.len() < MIN_QUERY_LEN {
        return Vec::new();
    }

    let mut hits = Vec::new();
    let walker = ignore::WalkBuilder::new(workspace)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(MAX_DEPTH))
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !name.contains(&q) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(workspace) else { continue };
        let rel_string = rel.to_string_lossy().replace('\\', "/");
        let display_name = entry.file_name().to_string_lossy().into_owned();
        hits.push(SearchHit { path: rel_string, name: display_name });
        if hits.len() >= MAX_HITS {
            break;
        }
    }
    hits
}

pub async fn api_files_search(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<SearchQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let root: PathBuf = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let canon = root.canonicalize().unwrap_or(root);

    // Run the walk on the blocking pool — the `ignore` walker is sync and a
    // big workspace would otherwise block the request executor. Cap with a
    // 30 s wall-clock timeout so a misclick on a deep non-git tree can't pin
    // a blocking thread indefinitely.
    let query = q.q.clone();
    let work = tokio::task::spawn_blocking(move || search(&canon, &query));
    let hits = match tokio::time::timeout(SEARCH_TIMEOUT, work).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(_) => return (StatusCode::REQUEST_TIMEOUT, "search timed out").into_response(),
    };
    (StatusCode::OK, Json(hits)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "oxi-search-test-{label}-{}-{}",
            std::process::id(),
            crate::db::now_ts()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn empty_or_short_query_returns_no_hits() {
        let ws = temp_ws("empty");
        std::fs::write(ws.join("foo.txt"), b"").unwrap();
        assert!(search(&ws, "").is_empty());
        assert!(search(&ws, "f").is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn finds_matching_filename_substring() {
        let ws = temp_ws("match");
        std::fs::write(ws.join("readme.md"), b"").unwrap();
        std::fs::write(ws.join("README.txt"), b"").unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/main.rs"), b"").unwrap();
        let hits = search(&ws, "readme");
        assert_eq!(hits.len(), 2, "should find both readme variants case-insensitively");
        let hits = search(&ws, "main");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains("src/"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn respects_gitignore() {
        let ws = temp_ws("ignore");
        std::fs::write(ws.join(".gitignore"), b"target/\nbuild.log\n").unwrap();
        std::fs::create_dir_all(ws.join("target/release")).unwrap();
        std::fs::write(ws.join("target/release/binary"), b"").unwrap();
        std::fs::write(ws.join("build.log"), b"").unwrap();
        std::fs::write(ws.join("src.rs"), b"").unwrap();
        // .gitignore is honored only inside a git repo per `ignore` crate
        // semantics — initialize a git dir marker so the walker activates it.
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        let hits = search(&ws, "binary");
        assert!(hits.is_empty(), "ignored target/ entries must not surface");
        let hits = search(&ws, "build");
        assert!(hits.is_empty(), "ignored build.log must not surface");
        let hits = search(&ws, "src");
        assert_eq!(hits.len(), 1, "tracked file must still be found");
        let _ = std::fs::remove_dir_all(&ws);
    }
}
