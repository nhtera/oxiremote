//! CORS layer for cross-origin SPA deployments.
//!
//! Embedded mode (SPA served by the agent on the same origin as the API) does
//! not need CORS — every fetch is same-origin and the browser never sends an
//! `Origin` header that would trigger a preflight. The Cloudflare Pages SPA
//! flow is different: the SPA is served from `oxiremote-web.pages.dev` while
//! the API lives on the agent's tunnel hostname, so every request is
//! cross-origin and modern browsers issue OPTIONS preflights for any non-
//! simple method or custom header (Authorization, X-OXI-CSRF, Content-Type).
//!
//! Behavior:
//!   - `OXI_CORS_ORIGINS` unset / empty → returns `None` and the layer is not
//!     installed; embedded-mode behavior is unchanged byte-for-byte.
//!   - Comma-separated list of origins → exact-match allowlist. No wildcards;
//!     the agent's threat model treats arbitrary origins as untrusted because
//!     a permissive CORS policy lets any page on the open web call the tunnel
//!     once the operator has paired their phone.
//!
//! The layer is installed OUTSIDE the security middleware chain so OPTIONS
//! preflights (which carry no cookie / Bearer / CSRF) are answered before
//! `tunnel_guard` / `api_key_guard` reject them. This is the standard CORS
//! integration pattern for axum / tower.

use axum::http::{HeaderName, HeaderValue, Method};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};

const CORS_ENV: &str = "OXI_CORS_ORIGINS";
const CSRF_HEADER: &str = "x-oxi-csrf";

/// Build a CORS layer from `OXI_CORS_ORIGINS`. Returns `None` when the env
/// var is unset or empty so callers can skip installing the layer in embedded
/// mode. Origins must be exact strings like `https://oxiremote-web.pages.dev`
/// — the comparison is case-sensitive, scheme-aware, and includes the port.
pub fn build_layer() -> Option<CorsLayer> {
    let raw = std::env::var(CORS_ENV).ok()?;
    build_layer_from(&raw)
}

/// Pure helper — extracted so tests don't have to mutate process env.
fn build_layer_from(raw: &str) -> Option<CorsLayer> {
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::PATCH,
                Method::OPTIONS,
            ])
            .allow_headers([
                HeaderName::from_static("authorization"),
                HeaderName::from_static("content-type"),
                HeaderName::from_static(CSRF_HEADER),
            ])
            // Cross-origin Pages SPA uses Bearer + `credentials: 'omit'`, so
            // allow_credentials stays false. Same-origin embedded mode keeps
            // its cookie flow because no Origin header → no preflight → CORS
            // layer is invisible to it.
            .max_age(Duration::from_secs(86_400)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_none() {
        assert!(build_layer_from("").is_none());
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert!(build_layer_from("   ,  ,").is_none());
    }

    #[test]
    fn populated_returns_layer() {
        assert!(
            build_layer_from("https://oxiremote-web.pages.dev,https://remote.erai.dev").is_some()
        );
    }

    #[test]
    fn malformed_origin_skipped_but_others_kept() {
        // Invalid header value (control char) is filtered; a real one
        // alongside it still produces a layer.
        assert!(build_layer_from("\u{0007}bad,https://oxiremote-web.pages.dev").is_some());
    }

    #[test]
    fn only_malformed_returns_none() {
        assert!(build_layer_from("\u{0007}only-bad").is_none());
    }
}
