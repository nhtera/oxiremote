use std::sync::Arc;

use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::files::{active_root, resolve_existing, resolve_new, validate_rel_path};
use crate::AppState;

#[derive(Deserialize)]
pub struct UploadQuery {
    #[serde(default)]
    ws_id: Option<i64>,
}

pub const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Serialize)]
struct UploadResponse {
    path: String,
    size: u64,
}

/// Streaming multipart upload. Expects fields:
///   `dir`  — relative destination directory under workspace root ("" = root)
///   `file` — the file payload (must come after `dir`)
///
/// Constant memory: we flush each chunk straight to disk.
pub async fn api_files_upload(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Response {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let root = match active_root(&state, q.ws_id) {
        Ok(r) => r,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let root = &root;
    let mut dest_dir: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "multipart error");
                return (StatusCode::BAD_REQUEST, "multipart error").into_response();
            }
        };

        match field.name() {
            Some("dir") => {
                let text = match field.text().await {
                    Ok(t) => t,
                    Err(_) => return (StatusCode::BAD_REQUEST, "invalid dir field").into_response(),
                };
                dest_dir = Some(text);
            }
            Some("file") => {
                let dir_rel = dest_dir.as_deref().unwrap_or("").to_string();
                let file_name = match field.file_name() {
                    Some(name) => sanitize_upload_name(name),
                    None => return (StatusCode::BAD_REQUEST, "missing filename").into_response(),
                };
                if file_name.is_empty() {
                    return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
                }

                let dir_path = if dir_rel.is_empty() {
                    match root.canonicalize() {
                        Ok(p) => p,
                        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "workspace invalid").into_response(),
                    }
                } else {
                    match resolve_existing(root, &dir_rel) {
                        Ok(p) if p.is_dir() => p,
                        Ok(_) => return (StatusCode::BAD_REQUEST, "dir is not a directory").into_response(),
                        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
                    }
                };

                // Re-validate joined path against workspace root.
                let rel_joined = if dir_rel.is_empty() {
                    file_name.clone()
                } else {
                    format!("{}/{}", dir_rel.trim_end_matches('/'), file_name)
                };
                if validate_rel_path(&rel_joined).is_err() {
                    return (StatusCode::BAD_REQUEST, "invalid upload path").into_response();
                }
                let target = match resolve_new(root, &rel_joined) {
                    Ok(p) => p,
                    Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
                };
                if target.exists() {
                    return (StatusCode::CONFLICT, "destination exists").into_response();
                }

                // Stream field to disk with running size cap.
                // Unique `.part` suffix prevents two concurrent uploads of the same
                // filename from clobbering each other's temp file.
                let unique: u64 = rand::random();
                let tmp = dir_path.join(format!(".{}.{:016x}.part", file_name, unique));
                let mut file = match fs::File::create(&tmp).await {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(error = %e, "create tmp failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "create failed").into_response();
                    }
                };

                let mut total: u64 = 0;
                let mut field = field;
                loop {
                    let chunk = match field.chunk().await {
                        Ok(Some(c)) => c,
                        Ok(None) => break,
                        Err(e) => {
                            let _ = fs::remove_file(&tmp).await;
                            warn!(error = %e, "chunk read failed");
                            return (StatusCode::BAD_REQUEST, "upload read failed").into_response();
                        }
                    };
                    total += chunk.len() as u64;
                    if total > MAX_UPLOAD_BYTES {
                        let _ = fs::remove_file(&tmp).await;
                        return (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds 100MB").into_response();
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        let _ = fs::remove_file(&tmp).await;
                        warn!(error = %e, "write chunk failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response();
                    }
                }
                if let Err(e) = file.flush().await {
                    let _ = fs::remove_file(&tmp).await;
                    warn!(error = %e, "flush failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "flush failed").into_response();
                }
                drop(file);

                if let Err(e) = fs::rename(&tmp, &target).await {
                    let _ = fs::remove_file(&tmp).await;
                    warn!(error = %e, "rename final failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "finalize failed").into_response();
                }

                return (
                    StatusCode::CREATED,
                    Json(UploadResponse {
                        path: rel_joined,
                        size: total,
                    }),
                )
                    .into_response();
            }
            _ => {
                // Ignore unknown fields so clients can add metadata later.
                let _ = field.bytes().await;
            }
        }
    }

    (StatusCode::BAD_REQUEST, "no file field").into_response()
}

/// Keep the basename only and strip characters that would be unsafe on any filesystem.
fn sanitize_upload_name(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    base.chars()
        .filter(|c| !matches!(c, '\0' | '\n' | '\r'))
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize_upload_name;

    #[test]
    fn sanitize_strips_path_segments() {
        assert_eq!(sanitize_upload_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_upload_name("C:\\Windows\\notes.txt"), "notes.txt");
        assert_eq!(sanitize_upload_name("clean.md"), "clean.md");
    }

    #[test]
    fn sanitize_strips_control_chars() {
        assert_eq!(sanitize_upload_name("bad\0name.txt"), "badname.txt");
    }
}
