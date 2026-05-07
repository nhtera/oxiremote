// Security headers middleware (Phase 05 / H5).
//
// Emits Content-Security-Policy and companion headers on every Public response.
// Localhost responses skip CSP — the agent dashboard at `/agent/*` is loopback-
// only and CSP gets in the way of its devtools usage.
//
// `connect-src` is explicit: `'self'` plus the discovery worker origin when
// `OXI_DISCOVERY_URL` is configured. No wildcards, no `wss:`, no `https:`.
// Resolved per-response from `AppState` so a config change picks up without
// restart (cheap — the URL parses to a string we cache in `connect_src_extra`
// at boot, see `mod.rs::AppState::csp_connect_src_extra`).
//
// The policy is enforcing from day one — no Report-Only stage. If a violation
// is reported in the field after merge we hotfix the policy.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

use super::route_scope::{RouteScope, scope_for_path};
use crate::AppState;

const X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
const REFERRER_POLICY: HeaderName = HeaderName::from_static("referrer-policy");

/// Build the CSP header value. `connect_src_extra` is the discovery worker
/// origin (e.g. `https://oxiremote-discovery.example.workers.dev`) or empty
/// when discovery is unconfigured.
pub(crate) fn build_csp(connect_src_extra: &str) -> String {
    let connect_src = if connect_src_extra.is_empty() {
        "'self'".to_string()
    } else {
        format!("'self' {connect_src_extra}")
    };
    format!(
        "default-src 'self'; \
         connect-src {connect_src}; \
         script-src 'self'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' blob: data:; \
         font-src 'self' data:; \
         worker-src 'self' blob:; \
         object-src 'none'; \
         frame-ancestors 'none'; \
         base-uri 'self'; \
         form-action 'self'"
    )
}

/// Parse `OXI_DISCOVERY_URL` into a CSP-safe origin (`https://host[:port]`).
/// Returns empty string when unset or malformed. Only `https://` is accepted —
/// CSP `connect-src` for plain http would be a downgrade attack vector.
pub fn discovery_origin_for_csp(url: Option<&str>) -> String {
    let raw = url.unwrap_or("").trim();
    let Some(rest) = raw.strip_prefix("https://") else {
        return String::new();
    };
    // Authority section ends at the first `/`, `?`, or `#`.
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains('@') || authority.contains(' ') {
        return String::new();
    }
    // Validate host[:port] shape: host must be non-empty and may have a numeric
    // port. We don't accept IDN here; the worker URL is operator-set and
    // ASCII-only in every realistic deploy.
    let (host, port_opt) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h, Some(p))
        }
        _ => (authority, None),
    };
    if host.is_empty() || host.contains(':') {
        // contains(':') would mean we failed to split — reject ambiguous input.
        return String::new();
    }
    match port_opt {
        Some(p) => format!("https://{host}:{p}"),
        None => format!("https://{host}"),
    }
}

pub async fn security_headers(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;

    // Localhost-scoped routes (the host dashboard at `/agent/*` and ops APIs)
    // skip CSP — they are loopback-only and intentionally permissive for
    // devtools / dev-iteration. Public (tunnel-reachable) responses get the
    // full headers.
    if scope_for_path(&path) == RouteScope::Localhost {
        return resp;
    }

    // Debug-build SSR pages (`/`, `/login`) embed bootstrap inline `<script>`
    // for the cargo-run pairing UX. They never ship in release (the SPA
    // fallback handles the same paths). Skip CSP so the inline scripts run;
    // release builds still get the full policy.
    #[cfg(debug_assertions)]
    if path == "/" || path == "/login" {
        return resp;
    }

    let extra = discovery_origin_for_csp(state.discovery_url.as_deref());
    let csp = build_csp(&extra);
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&csp) {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_self_only_when_discovery_unset() {
        let csp = build_csp("");
        assert!(csp.contains("connect-src 'self';"), "got: {csp}");
        assert!(!csp.contains("https://"));
    }

    #[test]
    fn csp_includes_discovery_origin_when_set() {
        let csp = build_csp("https://oxi.example.workers.dev");
        assert!(csp.contains("connect-src 'self' https://oxi.example.workers.dev;"));
    }

    #[test]
    fn csp_has_lockdown_directives() {
        let csp = build_csp("");
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("base-uri 'self'"));
        assert!(csp.contains("form-action 'self'"));
        assert!(csp.contains("default-src 'self'"));
    }

    #[test]
    fn csp_allows_blob_workers_and_data_images() {
        let csp = build_csp("");
        // remote-desktop tile rendering uses createImageBitmap on blobs;
        // SPA service worker bootstraps from same-origin blob URLs.
        assert!(csp.contains("worker-src 'self' blob:"));
        assert!(csp.contains("img-src 'self' blob: data:"));
    }

    #[test]
    fn discovery_origin_strips_path_and_query() {
        let o = discovery_origin_for_csp(Some(
            "https://oxi.example.workers.dev/api/session/lookup?k=abc",
        ));
        assert_eq!(o, "https://oxi.example.workers.dev");
    }

    #[test]
    fn discovery_origin_keeps_explicit_port() {
        let o = discovery_origin_for_csp(Some("https://oxi.example.workers.dev:8443/x"));
        assert_eq!(o, "https://oxi.example.workers.dev:8443");
    }

    #[test]
    fn discovery_origin_rejects_http_and_garbage() {
        assert_eq!(
            discovery_origin_for_csp(Some("http://oxi.example.workers.dev")),
            ""
        );
        assert_eq!(discovery_origin_for_csp(Some("not a url")), "");
        assert_eq!(discovery_origin_for_csp(Some("")), "");
        assert_eq!(discovery_origin_for_csp(None), "");
    }
}
