use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::warn;

use crate::auth::require_active_auth;
use crate::workspaces;
use crate::AppState;

/// Return the active browsing root for this request. If `ws_id` refers to a
/// registered workspace for this host, use its path; otherwise fall back to the
/// process-level workspace_root (OXI_WORKSPACE).
pub(crate) fn active_root(state: &AppState, ws_id: Option<i64>) -> Result<PathBuf, &'static str> {
    match ws_id {
        Some(id) => workspaces::lookup_path(&state.db_path, &state.host_info.host_id, id),
        None => Ok(state.workspace_root.clone()),
    }
}

// Text-editor open cap. Larger files must be downloaded instead of opened in CM6.
pub(crate) const MAX_TEXT_OPEN_BYTES: u64 = 1_048_576;
const BINARY_SNIFF_BYTES: usize = 8192;

fn unauthed() -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

fn bad_request(msg: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, msg).into_response()
}

/// Reject obviously-unsafe relative paths before touching the filesystem.
/// Final workspace-escape check still happens via canonicalize().
pub(crate) fn validate_rel_path(rel: &str) -> Result<(), &'static str> {
    if rel.is_empty() {
        return Err("empty path");
    }
    if rel.contains('\0') {
        return Err("invalid path (NUL byte)");
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err("absolute path not allowed");
    }
    for seg in rel.split(['/', '\\']) {
        if seg == ".." {
            return Err("parent traversal not allowed");
        }
    }
    Ok(())
}

/// Resolve a path whose final component must already exist under workspace root.
pub(crate) fn resolve_existing(root: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    validate_rel_path(rel)?;
    let canon_root = root.canonicalize().map_err(|_| "workspace root invalid")?;
    let resolved = root.join(rel).canonicalize().map_err(|_| "path not found")?;
    if !resolved.starts_with(&canon_root) {
        return Err("path escapes workspace");
    }
    Ok(resolved)
}

/// Resolve a target for create / write / rename-destination. Parent must exist under root;
/// final component may or may not exist yet. Returns the canonicalised parent joined with
/// the requested filename (so we never accidentally follow a symlinked parent out of workspace).
pub(crate) fn resolve_new(root: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    validate_rel_path(rel)?;
    let canon_root = root.canonicalize().map_err(|_| "workspace root invalid")?;
    let target = root.join(rel);
    let parent = target.parent().ok_or("invalid path")?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|_| "parent directory does not exist")?;
    if !canon_parent.starts_with(&canon_root) {
        return Err("path escapes workspace");
    }
    let file_name = target.file_name().ok_or("invalid filename")?;
    let name_str = file_name.to_str().ok_or("invalid filename")?;
    if name_str.is_empty() || name_str == "." || name_str == ".." {
        return Err("invalid filename");
    }
    if name_str.contains('/') || name_str.contains('\\') || name_str.contains('\0') {
        return Err("invalid filename");
    }
    Ok(canon_parent.join(file_name))
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn sniff_binary(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path).await else {
        return false;
    };
    let mut buf = [0u8; BINARY_SNIFF_BYTES];
    let n = f.read(&mut buf).await.unwrap_or(0);
    buf[..n].contains(&0u8)
}

fn os_err_response(e: std::io::Error) -> Response {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => (StatusCode::FORBIDDEN, "permission denied").into_response(),
        NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
        AlreadyExists => (StatusCode::CONFLICT, "already exists").into_response(),
        _ => {
            // Don't leak raw OS error strings (may contain absolute paths).
            warn!(error = %e, "file op I/O error");
            (StatusCode::INTERNAL_SERVER_ERROR, "I/O error").into_response()
        }
    }
}

// ---------- list ----------

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
    #[serde(default)]
    show_hidden: Option<bool>,
    #[serde(default)]
    ws_id: Option<i64>,
}

pub async fn api_files_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<ListQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }

    let root = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let dir = match q.path.as_deref().filter(|p| !p.is_empty()) {
        Some(rel) => match resolve_existing(&root, rel) {
            Ok(p) => p,
            Err(msg) => return bad_request(msg),
        },
        None => root.canonicalize().unwrap_or_else(|_| root.clone()),
    };

    if !dir.is_dir() {
        return bad_request("not a directory");
    }

    let mut entries = Vec::new();
    let mut read_dir = match fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => {
            warn!(error = %e, "read_dir failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let show_hidden = q.show_hidden.unwrap_or(false);
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        entries.push(FileEntry {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: mtime_secs(&meta),
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    (StatusCode::OK, Json(entries)).into_response()
}

// ---------- stat ----------

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
    #[serde(default)]
    pub ws_id: Option<i64>,
}

#[derive(Serialize)]
struct StatResponse {
    path: String,
    name: String,
    is_dir: bool,
    size: u64,
    modified: i64,
    is_binary: bool,
    mime: String,
}

pub async fn api_files_stat(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<PathQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let path = match resolve_existing(&root, &q.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    let Ok(meta) = fs::metadata(&path).await else {
        return bad_request("path not found");
    };
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_dir = meta.is_dir();
    let (is_binary, mime) = if is_dir {
        (false, String::from("inode/directory"))
    } else {
        let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
        (sniff_binary(&path).await, mime)
    };
    (
        StatusCode::OK,
        Json(StatResponse {
            path: q.path,
            name,
            is_dir,
            size: meta.len(),
            modified: mtime_secs(&meta),
            is_binary,
            mime,
        }),
    )
        .into_response()
}

// ---------- read (text, size-capped) ----------

#[derive(Serialize)]
struct ReadResponse {
    content: String,
    modified: i64,
    size: u64,
}

pub async fn api_files_read(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<PathQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let file_path = match resolve_existing(&root, &q.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    let Ok(meta) = fs::metadata(&file_path).await else {
        return bad_request("path not found");
    };
    if !meta.is_file() {
        return bad_request("not a file");
    }
    if meta.len() > MAX_TEXT_OPEN_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "file too large (max 1MB) — use download",
        )
            .into_response();
    }
    if sniff_binary(&file_path).await {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "binary file — use download",
        )
            .into_response();
    }
    let content = match fs::read_to_string(&file_path).await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid UTF-8 — use download",
            )
                .into_response();
        }
    };
    (
        StatusCode::OK,
        Json(ReadResponse {
            content,
            modified: mtime_secs(&meta),
            size: meta.len(),
        }),
    )
        .into_response()
}

// ---------- write (optimistic concurrency via if_mtime) ----------

#[derive(Deserialize)]
pub struct WriteRequest {
    path: String,
    content: String,
    #[serde(default)]
    if_mtime: Option<i64>,
    #[serde(default)]
    ws_id: Option<i64>,
}

#[derive(Serialize)]
struct WriteResponse {
    modified: i64,
    size: u64,
}

#[derive(Serialize)]
struct StaleError {
    error: &'static str,
    current_mtime: i64,
    current_size: u64,
}

pub async fn api_files_write(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<WriteRequest>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    if req.content.len() as u64 > MAX_TEXT_OPEN_BYTES {
        return bad_request("content too large (max 1MB)");
    }
    let root = match active_root(&state, req.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let target = match resolve_new(&root, &req.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };

    // Refuse to write through a symlink — stops TOCTOU-style escapes even though
    // resolve_new already validated the canonical parent.
    if let Ok(meta) = fs::symlink_metadata(&target).await {
        if meta.file_type().is_symlink() {
            return bad_request("refusing to write through symlink");
        }
        // Optimistic concurrency only meaningful when file already exists.
        if let Some(expected) = req.if_mtime {
            let current = mtime_secs(&meta);
            if current != expected {
                return (
                    StatusCode::CONFLICT,
                    Json(StaleError {
                        error: "stale",
                        current_mtime: current,
                        current_size: meta.len(),
                    }),
                )
                    .into_response();
            }
        }
    }

    if let Err(e) = fs::write(&target, &req.content).await {
        warn!(error = %e, "write file failed");
        return os_err_response(e);
    }
    let size = req.content.len() as u64;
    let modified = fs::metadata(&target).await.map(|m| mtime_secs(&m)).unwrap_or(0);
    (StatusCode::OK, Json(WriteResponse { modified, size })).into_response()
}

// ---------- create (file or dir) ----------

#[derive(Deserialize)]
pub struct CreateRequest {
    path: String,
    kind: String, // "file" | "dir"
    #[serde(default)]
    ws_id: Option<i64>,
}

pub async fn api_files_create(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<CreateRequest>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, req.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let target = match resolve_new(&root, &req.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    if target.exists() {
        return (StatusCode::CONFLICT, "already exists").into_response();
    }
    let res = match req.kind.as_str() {
        "file" => fs::write(&target, b"").await,
        "dir" => fs::create_dir(&target).await,
        _ => return bad_request("kind must be 'file' or 'dir'"),
    };
    match res {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            warn!(error = %e, "create failed");
            os_err_response(e)
        }
    }
}

// ---------- rename / move ----------

#[derive(Deserialize)]
pub struct RenameRequest {
    from: String,
    to: String,
    #[serde(default)]
    ws_id: Option<i64>,
}

pub async fn api_files_rename(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<RenameRequest>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, req.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let from = match resolve_existing(&root, &req.from) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    let to = match resolve_new(&root, &req.to) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    if to.exists() {
        return (StatusCode::CONFLICT, "destination exists").into_response();
    }
    match fs::rename(&from, &to).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            warn!(error = %e, "rename failed");
            os_err_response(e)
        }
    }
}

// ---------- delete ----------

pub async fn api_files_delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<PathQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, req.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let target = match resolve_existing(&root, &req.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    if target == canon_root {
        return bad_request("cannot delete workspace root");
    }
    let res = if target.is_dir() {
        fs::remove_dir_all(&target).await
    } else {
        fs::remove_file(&target).await
    };
    match res {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            warn!(error = %e, "delete failed");
            os_err_response(e)
        }
    }
}

// ---------- download (range-aware) ----------

pub async fn api_files_download(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return unauthed();
    }
    let root = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    let path = match resolve_existing(&root, &q.path) {
        Ok(p) => p,
        Err(msg) => return bad_request(msg),
    };
    let meta = match fs::metadata(&path).await {
        Ok(m) => m,
        Err(_) => return bad_request("path not found"),
    };
    if !meta.is_file() {
        return bad_request("not a file");
    }
    let total = meta.len();
    let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        sanitize_filename(&path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
    );

    let mut file = match fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if let Some((start, end)) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, total))
    {
        if total == 0 || start >= total {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        let end = end.min(total - 1);
        if end < start {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        let len = end - start + 1;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let stream = ReaderStream::new(file.take(len));
        let body = Body::from_stream(stream);
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, &mime)
            .header(header::CONTENT_LENGTH, len)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_DISPOSITION, &disposition)
            .body(body)
            .unwrap();
    }

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, total)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(body)
        .unwrap()
}

/// Parse a single `bytes=start-end` / `bytes=start-` / `bytes=-suffix` range.
/// `total` is the file length, needed to resolve suffix ranges per RFC 9110 §14.1.2.
fn parse_range(h: &str, total: u64) -> Option<(u64, u64)> {
    let s = h.strip_prefix("bytes=")?;
    // Single-range form only; comma means multi-range which we don't support.
    if s.contains(',') {
        return None;
    }
    if let Some(suffix) = s.strip_prefix('-') {
        let n: u64 = suffix.parse().ok()?;
        if n == 0 || total == 0 {
            return None;
        }
        let n = n.min(total);
        return Some((total - n, total - 1));
    }
    let (start, end) = s.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end: u64 = if end.is_empty() {
        u64::MAX
    } else {
        end.parse().ok()?
    };
    Some((start, end))
}

fn sanitize_filename(name: &str) -> String {
    // Strip anything that would alter the Content-Disposition header semantics.
    name.chars()
        .filter(|&c| c != '"' && c != '\\' && c != '\n' && c != '\r' && c != ';')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        assert!(validate_rel_path("../etc/passwd").is_err());
        assert!(validate_rel_path("foo/../bar").is_err());
        assert!(validate_rel_path("a/b/..").is_err());
    }

    #[test]
    fn rejects_absolute() {
        assert!(validate_rel_path("/etc/passwd").is_err());
        assert!(validate_rel_path("\\windows\\system32").is_err());
    }

    #[test]
    fn rejects_nul_and_empty() {
        assert!(validate_rel_path("").is_err());
        assert!(validate_rel_path("foo\0bar").is_err());
    }

    #[test]
    fn accepts_safe_paths() {
        assert!(validate_rel_path("src/main.rs").is_ok());
        assert!(validate_rel_path("a.b.c").is_ok());
        assert!(validate_rel_path("deep/nested/dir/file.txt").is_ok());
    }

    fn temp_ws(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "oxi-files-test-{label}-{}-{}",
            std::process::id(),
            crate::db::now_ts()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn resolve_existing_rejects_workspace_escape() {
        let ws = temp_ws("esc");
        assert!(resolve_existing(&ws, "..").is_err());
        assert!(resolve_existing(&ws, "missing.txt").is_err());
        std::fs::write(ws.join("ok.txt"), b"hi").unwrap();
        assert!(resolve_existing(&ws, "ok.txt").is_ok());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlink_escape() {
        let ws = temp_ws("sym");
        let outside = std::env::temp_dir().join(format!(
            "oxi-outside-{}-{}",
            std::process::id(),
            crate::db::now_ts()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
        assert!(resolve_existing(&ws, "link/secret").is_err());
        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn parse_range_handles_all_forms() {
        // open-ended
        assert_eq!(parse_range("bytes=0-", 100), Some((0, u64::MAX)));
        // closed
        assert_eq!(parse_range("bytes=0-99", 100), Some((0, 99)));
        // suffix (last N bytes)
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        // suffix larger than file → clamp to full file
        assert_eq!(parse_range("bytes=-200", 50), Some((0, 49)));
        // zero-length suffix invalid
        assert_eq!(parse_range("bytes=-0", 50), None);
        // multi-range not supported
        assert_eq!(parse_range("bytes=0-10,20-30", 100), None);
        // garbage
        assert_eq!(parse_range("garbage", 100), None);
    }

    #[test]
    fn resolve_new_rejects_dotdot_and_bad_names() {
        let ws = temp_ws("new");
        assert!(resolve_new(&ws, "../outside").is_err());
        assert!(resolve_new(&ws, ".").is_err());
        assert!(resolve_new(&ws, "sub/..").is_err());
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        assert!(resolve_new(&ws, "sub/new.txt").is_ok());
        let _ = std::fs::remove_dir_all(&ws);
    }
}
