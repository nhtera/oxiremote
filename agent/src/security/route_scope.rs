// Compile-time route classification: is a given request path allowed to be
// reached via the Cloudflare tunnel (Public) or must it originate from the
// local machine (Localhost)?
//
// A request is considered tunnel-origin iff the request carries the
// `cf-connecting-ip` header, which cloudflared always injects on proxied
// traffic and which direct localhost callers (same-machine CLI, loopback
// browser) never have.
//
// The scope table is path-prefix based. New routes default to `Public` — if
// you add a route that must never be tunnel-reachable, add a matching prefix
// to `LOCALHOST_PREFIXES` and the test at the bottom of this file.

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteScope {
    /// Tunnel-accessible. Still requires auth wherever the handler asks for it.
    Public,
    /// Loopback-only. Tunnel-origin requests get 403.
    Localhost,
}

/// Path prefixes that are loopback-only.
const LOCALHOST_PREFIXES: &[&str] = &[
    "/api/notify",       // CLI-driven push fan-out (shared-secret bearer)
    "/api/local-sites",  // leaks listening-port enumeration on the host
    "/api/agent",        // agent dashboard API — host-only (events SSE, state)
    "/agent",            // agent dashboard SPA pages — host-only
];

pub fn scope_for_path(path: &str) -> RouteScope {
    if LOCALHOST_PREFIXES.iter().any(|p| path == *p || path.starts_with(&format!("{p}/"))) {
        RouteScope::Localhost
    } else {
        RouteScope::Public
    }
}

/// Tunnel-origin heuristic. cloudflared always sets `cf-connecting-ip`.
/// Loopback requests never have it.
///
/// # Threat model assumption
///
/// This detection is only sound when the agent binds exclusively to the
/// loopback interface (`127.0.0.1`). If the agent were exposed on a
/// non-loopback address, an adversary on the same LAN could forge the
/// `cf-connecting-ip` header and bypass the Localhost route guard.
/// A `debug_assert!` in `main` enforces the loopback-only invariant at
/// startup so deviations are caught immediately during development.
pub fn is_tunnel_request(headers: &HeaderMap) -> bool {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .is_some_and(|v| !v.is_empty())
}

/// Axum middleware. Rejects tunnel-origin requests to `Localhost`-scoped
/// paths with 403 before the handler runs.
pub async fn tunnel_guard(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if scope_for_path(path) == RouteScope::Localhost && is_tunnel_request(req.headers()) {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::from("localhost-only route"))
            .unwrap();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn public_routes_default() {
        assert_eq!(scope_for_path("/api/health"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/me"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/terminal/sessions"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/files/list"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/previews"), RouteScope::Public);
        assert_eq!(scope_for_path("/preview/abc"), RouteScope::Public);
        assert_eq!(scope_for_path("/"), RouteScope::Public);
    }

    #[test]
    fn localhost_only_routes_are_classified() {
        assert_eq!(scope_for_path("/api/notify"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/api/notify/extra"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/api/local-sites"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/api/agent"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/api/agent/events"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/api/agent/state"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/agent"), RouteScope::Localhost);
        assert_eq!(scope_for_path("/agent/devices"), RouteScope::Localhost);
    }

    #[test]
    fn prefix_boundary_not_over_matched() {
        // `/api/notifycheater` must not be classified as Localhost — the
        // match has to respect segment boundaries.
        assert_eq!(scope_for_path("/api/notifycheater"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/local-sites-public"), RouteScope::Public);
        assert_eq!(scope_for_path("/api/agentless"), RouteScope::Public);
        assert_eq!(scope_for_path("/agent-about"), RouteScope::Public);
    }

    #[test]
    fn tunnel_detection_reads_cf_header() {
        let mut headers = HeaderMap::new();
        assert!(!is_tunnel_request(&headers));

        headers.insert("cf-connecting-ip", HeaderValue::from_static(""));
        assert!(!is_tunnel_request(&headers));

        headers.insert("cf-connecting-ip", HeaderValue::from_static("1.2.3.4"));
        assert!(is_tunnel_request(&headers));
    }
}
