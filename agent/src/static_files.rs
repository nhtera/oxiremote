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
        None => {
            #[cfg(debug_assertions)]
            {
                return debug_help_page();
            }
            #[cfg(not(debug_assertions))]
            {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
    }
}

/// Friendly first-run page shown in debug builds when `apps/web/dist/` hasn't
/// been built yet. Explains the two options (`bun run build:web` for
/// standalone, `bun dev` for Vite hot-reload) and lets the user open the
/// Vite URL in one click. Never reachable in release builds (build.rs
/// panics if dist is missing).
#[cfg(debug_assertions)]
fn debug_help_page() -> Response {
    let html = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>OxiRemote — SPA not built</title>
  <style>
    :root { color-scheme: dark; }
    body {
      font-family: -apple-system, system-ui, sans-serif;
      background: #16171d; color: #f3f4f6;
      margin: 0; padding: 48px 24px; min-height: 100vh;
      display: flex; align-items: center; justify-content: center;
    }
    .card {
      max-width: 560px; background: #1f2028;
      border: 1px solid #2e303a; border-radius: 12px;
      padding: 32px;
    }
    h1 { margin: 0 0 8px; font-size: 1.5rem; }
    p { color: #9ca3af; line-height: 1.6; }
    code {
      background: #0f1014; color: #6cb4ff;
      padding: 2px 6px; border-radius: 4px;
      font-family: ui-monospace, Menlo, monospace; font-size: 0.9em;
    }
    .options { display: grid; gap: 16px; margin-top: 24px; }
    .option {
      background: #16171d; border: 1px solid #2e303a;
      border-radius: 8px; padding: 16px;
    }
    .option strong { display: block; margin-bottom: 4px; }
    .option .body { color: #9ca3af; font-size: 0.9rem; }
    a {
      color: #6cb4ff; text-decoration: none;
      font-weight: 500;
    }
    a:hover { text-decoration: underline; }
  </style>
</head>
<body>
  <div class="card">
    <h1>SPA not built yet</h1>
    <p>The web frontend hasn't been compiled. Pick one of the options below to continue.</p>
    <div class="options">
      <div class="option">
        <strong>Standalone agent (this port)</strong>
        <div class="body">
          Run <code>bun run build:web</code> once, then refresh this page.
          The agent will serve the SPA directly from <code>apps/web/dist</code>.
        </div>
      </div>
      <div class="option">
        <strong>Hot-reload development</strong>
        <div class="body">
          Run <code>bun dev</code> from the repo root, then open
          <a href="http://localhost:5173/">http://localhost:5173/</a>.
          Vite will proxy <code>/api</code>, <code>/ws</code>, and
          <code>/preview</code> to the agent on port 8787.
        </div>
      </div>
    </div>
  </div>
</body>
</html>"#;
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        html,
    )
        .into_response()
}

pub async fn spa_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path.starts_with("/preview/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = path.trim_start_matches('/');
    serve_asset(path).unwrap_or_else(spa_fallback)
}
