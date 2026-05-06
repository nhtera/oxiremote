use std::path::{Path, PathBuf};
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
    /// `"suffix"` walks `name (1).ext`, `name (2).ext`, ... when the destination
    /// exists; default (`None` / `"fail"`) returns 409 — preserves existing
    /// files-page semantics where a conflict is a real user-actionable error.
    /// Chat-style attach flows (paste/drop/picker/desktop modal) opt in.
    #[serde(default)]
    on_conflict: Option<String>,
}

/// Maximum suffix attempts when on_conflict=suffix. Bounded so a hostile or
/// runaway client can't burn unbounded I/O probing the filesystem.
const MAX_SUFFIX_ATTEMPTS: u32 = 999;

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
                    // Auto-mkdir for chat-style attach flows. The SPA used to
                    // POST /api/files/create first, then upload — but the
                    // pre-flight silently failed cross-origin (CORS, transient
                    // 5xx, race) and the upload fell back to workspace root,
                    // scattering screenshots there. Reject ".." traversal up
                    // front, then create_dir_all and re-resolve so canonical
                    // form is checked against workspace root.
                    if let Err(msg) = validate_rel_path(&dir_rel) {
                        return (StatusCode::BAD_REQUEST, msg).into_response();
                    }
                    let candidate = root.join(&dir_rel);
                    if !candidate.exists()
                        && let Err(e) = fs::create_dir_all(&candidate).await
                    {
                        warn!(error = %e, "auto-mkdir upload dir failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "mkdir failed").into_response();
                    }
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

                // Resolve the final on-disk name. With `on_conflict=suffix` the
                // server walks `name (1).ext`, `name (2).ext`, ... — useful for
                // chat-style attach flows where the user re-uploads the same
                // photo from their library. Default (`fail`) preserves the
                // existing 409 contract for files-page uploads.
                let suffix_on_conflict = matches!(q.on_conflict.as_deref(), Some("suffix"));
                let (final_name, final_rel, final_target) = if target.exists() {
                    if !suffix_on_conflict {
                        return (StatusCode::CONFLICT, "destination exists").into_response();
                    }
                    match next_free_suffix(&dir_path, root, &dir_rel, &file_name) {
                        Some(t) => t,
                        None => {
                            return (StatusCode::CONFLICT, "destination exists").into_response();
                        }
                    }
                } else {
                    (file_name.clone(), rel_joined.clone(), target.clone())
                };

                // Stream field to disk with running size cap.
                // Unique `.part` suffix prevents two concurrent uploads of the same
                // filename from clobbering each other's temp file.
                let unique: u64 = rand::random();
                let tmp = dir_path.join(format!(".{}.{:016x}.part", final_name, unique));
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

                if let Err(e) = fs::rename(&tmp, &final_target).await {
                    let _ = fs::remove_file(&tmp).await;
                    warn!(error = %e, "rename final failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "finalize failed").into_response();
                }

                return (
                    StatusCode::CREATED,
                    Json(UploadResponse {
                        path: final_rel,
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

/// Walks `name (1).ext`, `name (2).ext`, ... within `dir_path` and returns the
/// first available `(name, relative_path, absolute_target)`. Stops after
/// `MAX_SUFFIX_ATTEMPTS` to bound runaway probing if the directory is somehow
/// pre-filled with every variant.
fn next_free_suffix(
    dir_path: &Path,
    root: &Path,
    dir_rel: &str,
    file_name: &str,
) -> Option<(String, String, PathBuf)> {
    let (stem, ext) = split_stem_ext(file_name);
    for n in 1..=MAX_SUFFIX_ATTEMPTS {
        let candidate = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate_target = dir_path.join(&candidate);
        if candidate_target.exists() {
            continue;
        }
        // Build the workspace-relative path for the response and validate it
        // canonicalises back inside the workspace root (defence-in-depth — the
        // suffix-walker should never escape the parent dir).
        let candidate_rel = if dir_rel.is_empty() {
            candidate.clone()
        } else {
            format!("{}/{}", dir_rel.trim_end_matches('/'), candidate)
        };
        if validate_rel_path(&candidate_rel).is_err() {
            continue;
        }
        // `resolve_new` re-canonicalises against the workspace root and
        // rejects paths that escape — defence-in-depth even though the
        // suffix walker only mutates the basename.
        if let Ok(resolved) = resolve_new(root, &candidate_rel) {
            return Some((candidate, candidate_rel, resolved));
        }
    }
    None
}

/// Returns `("name", "ext")`. Hidden files like `.bashrc` keep the leading
/// dot in stem and have empty ext, matching `Path::file_stem` semantics.
fn split_stem_ext(file_name: &str) -> (String, String) {
    let p = Path::new(file_name);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name)
        .to_string();
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    (stem, ext)
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
    use super::{next_free_suffix, sanitize_upload_name, split_stem_ext};

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

    #[test]
    fn split_stem_ext_handles_common_cases() {
        assert_eq!(split_stem_ext("image.jpg"), ("image".into(), "jpg".into()));
        assert_eq!(
            split_stem_ext("archive.tar.gz"),
            ("archive.tar".into(), "gz".into())
        );
        assert_eq!(split_stem_ext("README"), ("README".into(), "".into()));
        assert_eq!(split_stem_ext(".bashrc"), (".bashrc".into(), "".into()));
    }

    fn fresh_tmpdir(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "oxi-upload-{}-{}-{}",
            label,
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn next_free_suffix_finds_first_gap() {
        let root = fresh_tmpdir("first-gap");
        // Pre-fill: image.jpg, image (1).jpg → suffix should yield `image (2).jpg`.
        std::fs::write(root.join("image.jpg"), b"a").unwrap();
        std::fs::write(root.join("image (1).jpg"), b"b").unwrap();
        let (name, rel, target) =
            next_free_suffix(&root, &root, "", "image.jpg").expect("expected free slot");
        assert_eq!(name, "image (2).jpg");
        assert_eq!(rel, "image (2).jpg");
        // `target` comes from resolve_new which canonicalises (e.g. /tmp →
        // /private/tmp on macOS); compare via canonicalised root.
        let canon_root = root.canonicalize().unwrap();
        assert_eq!(target, canon_root.join("image (2).jpg"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn next_free_suffix_handles_subdir() {
        let root = fresh_tmpdir("subdir");
        std::fs::create_dir(root.join(".attachments")).unwrap();
        std::fs::write(root.join(".attachments").join("photo.png"), b"x").unwrap();
        let (name, rel, _target) = next_free_suffix(
            &root.join(".attachments"),
            &root,
            ".attachments",
            "photo.png",
        )
        .expect("expected free slot");
        assert_eq!(name, "photo (1).png");
        assert_eq!(rel, ".attachments/photo (1).png");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn next_free_suffix_extensionless() {
        let root = fresh_tmpdir("noext");
        std::fs::write(root.join("README"), b"x").unwrap();
        let (name, _rel, _target) =
            next_free_suffix(&root, &root, "", "README").expect("expected free slot");
        assert_eq!(name, "README (1)");
        std::fs::remove_dir_all(&root).ok();
    }
}
