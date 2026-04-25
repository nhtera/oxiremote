// Tunnel-side Bearer verification.
//
// Flow:
//   - Loopback requests: pass through; cookie auth alone is sufficient.
//   - Tunnel-origin requests (cf-connecting-ip present):
//       * `Authorization: Bearer <api_key>` is mandatory on authenticated routes.
//       * Missing / invalid → 401.
//       * The pairing + vapid-public endpoints are exempt (clients can't yet
//         hold an API key before pairing, and the VAPID public key is public).
//
// This layer sits AFTER `tunnel_guard` (Localhost routes already rejected) and
// AFTER `csrf_guard`, so only tunnel-allowed, CSRF-clean requests reach us.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::auth::verify_api_key_async;
use crate::AppState;

use super::route_scope::is_tunnel_request;

const EXEMPT_PREFIXES: &[&str] = &[
    "/api/health",
    "/api/pairing/exchange",
    "/api/login/one-time",       // OTK login issues the session; no key yet
    "/api/auth/approval-status", // pending-device polling; uses cookie auth in handler
    "/api/push/vapid-public",
    "/preview/",
    "/proxy/",  // local-sites reverse proxy; auth checked inside the handler
    "/assets/", // hashed static assets; /api/ namespace not allowed here
    "/login",
];

/// `/api/evil.js` must NOT be exempt just because it ends in `.js`; we only
/// allow bare-file requests for a small set of SPA-root filenames.
const EXEMPT_ROOT_FILES: &[&str] = &[
    "/favicon.ico",
    "/favicon.svg",
    "/manifest.webmanifest",
    "/sw.js",
    "/robots.txt",
];

fn is_exempt(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    if EXEMPT_PREFIXES.iter().any(|p| path == *p || path.starts_with(*p)) {
        return true;
    }
    EXEMPT_ROOT_FILES.contains(&path)
}

fn bearer(req: &Request) -> Option<String> {
    let raw = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    raw.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

pub async fn api_key_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if !is_tunnel_request(req.headers()) || is_exempt(req.uri().path()) {
        return next.run(req).await;
    }

    let Some(key) = bearer(&req) else {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("missing api key"))
            .unwrap();
    };

    if verify_api_key_async(state.db_path.clone(), key).await.is_none() {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("invalid api key"))
            .unwrap();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exempt_paths_pass() {
        assert!(is_exempt("/api/health"));
        assert!(is_exempt("/api/pairing/exchange"));
        assert!(is_exempt("/api/login/one-time"));
        assert!(is_exempt("/api/auth/approval-status"));
        assert!(is_exempt("/api/push/vapid-public"));
        assert!(is_exempt("/preview/abc"));
        assert!(is_exempt("/proxy/3000/index.html"));
        assert!(is_exempt("/assets/index-abc.js"));
        assert!(is_exempt("/"));
        assert!(is_exempt("/manifest.webmanifest"));
        assert!(is_exempt("/sw.js"));
    }

    #[test]
    fn non_exempt_paths_need_key() {
        assert!(!is_exempt("/api/me"));
        assert!(!is_exempt("/api/terminal/sessions"));
        assert!(!is_exempt("/api/files/list"));
    }

    #[test]
    fn suffix_bypass_is_closed() {
        // Key finding from code review: `.js` suffix must not smuggle past
        // the guard. Only /assets/ files and a small allow-list of root
        // files are exempt.
        assert!(!is_exempt("/api/evil.js"));
        assert!(!is_exempt("/api/files/list.js"));
        assert!(!is_exempt("/api/admin.webmanifest"));
    }
}
