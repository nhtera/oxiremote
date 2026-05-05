//! Pairing-time origin capture for the runtime CORS allowlist.
//!
//! After a successful pairing exchange (or OTK login), the SPA tab's `Origin`
//! header is added to the agent's CORS allowlist so that the same SPA can
//! later cross-origin fetch this agent's API. The trust transitivity is:
//! the SPA presented a valid pairing credential → its origin can reach the
//! agent's tunnel-facing API for the lifetime of the issued device key.
//!
//! Origins are validated minimally: must be `http://` or `https://`, no
//! wildcards, no trailing slash. The browser-supplied `Origin` header cannot
//! be forged from another origin; this is the same trust chain that standard
//! CORS preflight relies on.

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, header};

use crate::AppState;
use crate::db;
use crate::security::cors::CorsOrigins;

/// Inspect the request's `Origin` header and add it to the runtime allowlist,
/// then persist to DB. Silently no-op when the header is missing or malformed
/// — non-browser pairing flows (CLI tools, server-to-server) have no Origin.
pub fn capture_pairing_origin(headers: &HeaderMap, state: &Arc<AppState>) {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return;
    };
    let Ok(origin_str) = origin.to_str() else {
        return;
    };
    if !is_well_formed_origin(origin_str) {
        return;
    }
    insert_origin(&state.cors_origins, origin.clone());

    let db_path = state.db_path.clone();
    let origin_owned = origin_str.to_string();
    // DB write off the request hot path. Failure is logged inside but never
    // propagated — the in-memory set is the source of truth for the running
    // process; DB is for persistence across restarts.
    tokio::task::spawn_blocking(move || {
        if let Err(err) = db::upsert_cors_origin(&db_path, &origin_owned) {
            tracing::warn!(error=%err, origin=%origin_owned, "failed to persist cors origin");
        }
    });
}

fn is_well_formed_origin(s: &str) -> bool {
    if s.is_empty() || s.contains('*') || s.ends_with('/') {
        return false;
    }
    s.starts_with("https://") || s.starts_with("http://")
}

fn insert_origin(set: &CorsOrigins, value: HeaderValue) {
    if let Ok(mut guard) = set.write() {
        guard.insert(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_accepts_https_and_http() {
        assert!(is_well_formed_origin("https://abc.trycloudflare.com"));
        assert!(is_well_formed_origin("http://localhost:5173"));
    }

    #[test]
    fn well_formed_rejects_wildcards_and_slash() {
        assert!(!is_well_formed_origin("https://*.example.com"));
        assert!(!is_well_formed_origin("https://example.com/"));
        assert!(!is_well_formed_origin("https://"));
    }

    #[test]
    fn well_formed_rejects_non_http() {
        assert!(!is_well_formed_origin("file:///"));
        assert!(!is_well_formed_origin("null"));
        assert!(!is_well_formed_origin(""));
    }
}
