use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../apps/web/dist"]
struct WebAssets;

fn serve_asset(path: &str) -> Option<Response> {
    let file = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let cache = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Some(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            file.data.to_vec(),
        )
            .into_response(),
    )
}

fn spa_fallback() -> Response {
    match WebAssets::get("index.html") {
        Some(file) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            file.data.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn spa_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path.starts_with("/preview/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = path.trim_start_matches('/');
    serve_asset(path).unwrap_or_else(spa_fallback)
}
