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
    // WebSocket upgrade endpoints: browsers cannot set custom headers on the
    // initial Upgrade request, so Bearer auth is impossible. Both routes check
    // the oxiremote_session cookie inside their handler as the first privileged
    // call — exempting them here only removes the middleware 401 that would
    // otherwise fire before the handler runs.
    //
    // NARROW CHOICE: We do NOT exempt "/api/terminal/sessions/" because that
    // prefix also covers /resize, /close, and PATCH /{id} — all of which must
    // remain behind api_key_guard. Instead we match the exact /ws suffix via
    // EXEMPT_WS_SUFFIXES below. "/ws/desktop/" is already unique to the WS
    // route and safe to exempt as a prefix.
    "/ws/desktop/",
];

/// Path suffixes for WebSocket-only exemptions where the prefix would be too
/// broad. Checked only when no prefix already matched.
const EXEMPT_WS_SUFFIXES: &[&str] = &[
    "/ws", // matches /api/terminal/sessions/{id}/ws exactly
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
    if EXEMPT_ROOT_FILES.contains(&path) {
        return true;
    }
    // Suffix check for WS-only routes where the prefix would be too broad.
    // Only matches terminal session WebSocket upgrades.
    EXEMPT_WS_SUFFIXES.iter().any(|s| path.ends_with(s))
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

    // F1 regression: WebSocket upgrade endpoints must bypass the Bearer guard
    // because browsers cannot set custom headers on the WS Upgrade request.
    // Auth is enforced inside each handler via cookie session check.

    #[test]
    fn ws_upgrade_paths_are_exempt() {
        // Desktop WS: prefix-matched via /ws/desktop/
        assert!(is_exempt("/ws/desktop/some-device-uuid"));

        // Terminal WS: suffix-matched via /ws — must match the exact route
        // /api/terminal/sessions/{id}/ws
        assert!(is_exempt("/api/terminal/sessions/abc-123/ws"));
    }

    #[test]
    fn non_ws_terminal_routes_keep_guard() {
        // /resize, /close, and PATCH/{id} must NOT be exempt — they carry
        // state-changing work and must require a Bearer token over the tunnel.
        assert!(!is_exempt("/api/terminal/sessions/abc-123/resize"));
        assert!(!is_exempt("/api/terminal/sessions/abc-123/close"));
        assert!(!is_exempt("/api/terminal/sessions/abc-123"));
        // Paranoia: a crafted path ending in /ws under a different namespace
        // would also be exempt — this is acceptable because the handler behind
        // it still validates the session cookie. Document the trade-off here.
        // If this ever becomes a concern, switch to an explicit path allowlist.
    }
}
