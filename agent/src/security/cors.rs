//! Runtime-mutable CORS layer for multi-host SPA deployments.
//!
//! Two scenarios share this layer:
//!   - Discovery mode (Cloudflare Pages SPA on a stable origin, agent on a
//!     tunnel) — origins seeded at boot from `OXI_CORS_ORIGINS`.
//!   - Embedded multi-host (SPA tab on host A's tunnel cross-origin fetches
//!     host B's API after both have been paired from the same SPA) — origins
//!     captured at pairing time and persisted to the `settings` table.
//!
//! The allowlist lives behind `Arc<RwLock<HashSet<HeaderValue>>>` so handlers
//! can mutate it without rebuilding the router. Same-origin embedded mode is
//! unaffected: browsers don't send an `Origin` header on same-origin fetches,
//! so the layer is invisible to that flow.
//!
//! Preflight OPTIONS responses are short-circuited before reaching the security
//! chain (`tunnel_guard` / `csrf_guard` / `api_key_guard`) — preflights carry
//! no auth credentials by design.

use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
    task::{Context, Poll},
};

use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use futures_util::future::BoxFuture;
use tower::{Layer, Service};

const ALLOWED_METHODS: &str = "GET, POST, PUT, DELETE, PATCH, OPTIONS";
const ALLOWED_HEADERS: &str = "authorization, content-type, x-oxi-csrf";
const MAX_AGE: &str = "86400";
const CORS_ENV: &str = "OXI_CORS_ORIGINS";

/// Shared runtime-mutable allowlist of CORS origins.
pub type CorsOrigins = Arc<RwLock<HashSet<HeaderValue>>>;

/// Build the shared origin set. `OXI_CORS_ORIGINS` (env, backward compat) and
/// the persisted DB list are merged on boot. After this point the set is
/// mutated only by pairing handlers and the localhost CORS CRUD endpoints.
pub fn seed_origins(env_origins: &[String], db_origins: &[String]) -> CorsOrigins {
    let mut set: HashSet<HeaderValue> = HashSet::new();
    for s in env_origins.iter().chain(db_origins.iter()) {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = HeaderValue::from_str(trimmed) {
            set.insert(v);
        }
    }
    Arc::new(RwLock::new(set))
}

/// Read the seed origins from `OXI_CORS_ORIGINS` (comma-separated). Empty when
/// unset. Kept here so `main.rs` doesn't have to re-implement the parsing.
pub fn env_origins() -> Vec<String> {
    std::env::var(CORS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Tower layer that wraps any service with the runtime CORS check.
#[derive(Clone)]
pub struct CorsLayer {
    origins: CorsOrigins,
}

impl CorsLayer {
    pub fn new(origins: CorsOrigins) -> Self {
        Self { origins }
    }
}

impl<S> Layer<S> for CorsLayer {
    type Service = CorsMiddleware<S>;
    fn layer(&self, inner: S) -> Self::Service {
        CorsMiddleware {
            inner,
            origins: self.origins.clone(),
        }
    }
}

#[derive(Clone)]
pub struct CorsMiddleware<S> {
    inner: S,
    origins: CorsOrigins,
}

impl<S, B> Service<axum::http::Request<B>> for CorsMiddleware<S>
where
    S: Service<axum::http::Request<B>, Response = axum::response::Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = axum::response::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: axum::http::Request<B>) -> Self::Future {
        let origins = self.origins.clone();
        let origin_header = req.headers().get(header::ORIGIN).cloned();
        let is_preflight = *req.method() == Method::OPTIONS;
        // Clone-and-swap pattern: the future owns the ready service while a
        // fresh clone takes its place for the next call. `poll_ready` was
        // already invoked on `self.inner`, so swapping after-the-fact is the
        // tower-recommended way to satisfy the 'static future bound.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            // Preflight: short-circuit. A preflight without an allowed origin
            // gets a 204 with no ACAO — the browser then blocks the actual
            // request, which is the correct CORS behaviour.
            if is_preflight {
                let mut resp = axum::response::Response::new(axum::body::Body::empty());
                *resp.status_mut() = StatusCode::NO_CONTENT;
                if let Some(origin) = &origin_header {
                    let allowed = origins
                        .read()
                        .map(|s| s.contains(origin))
                        .unwrap_or(false);
                    if allowed {
                        attach_cors_headers(resp.headers_mut(), origin);
                    }
                }
                return Ok(resp);
            }

            let mut resp = inner.call(req).await?;
            // Mirror standard CORS behaviour: emit ACAO on actual responses
            // when the request was cross-origin and the origin is allowed.
            if let Some(origin) = &origin_header {
                let allowed = origins
                    .read()
                    .map(|s| s.contains(origin))
                    .unwrap_or(false);
                if allowed {
                    resp.headers_mut().insert(
                        HeaderName::from_static("access-control-allow-origin"),
                        origin.clone(),
                    );
                }
            }
            Ok(resp)
        })
    }
}

fn attach_cors_headers(headers: &mut axum::http::HeaderMap, origin: &HeaderValue) {
    headers.insert(
        HeaderName::from_static("access-control-allow-origin"),
        origin.clone(),
    );
    headers.insert(
        HeaderName::from_static("access-control-allow-methods"),
        HeaderValue::from_static(ALLOWED_METHODS),
    );
    headers.insert(
        HeaderName::from_static("access-control-allow-headers"),
        HeaderValue::from_static(ALLOWED_HEADERS),
    );
    headers.insert(
        HeaderName::from_static("access-control-max-age"),
        HeaderValue::from_static(MAX_AGE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_merges_env_and_db_origins() {
        let env = vec!["https://a.example".to_string()];
        let db = vec!["https://b.example".to_string()];
        let set = seed_origins(&env, &db);
        let guard = set.read().unwrap();
        assert!(guard.contains(&HeaderValue::from_static("https://a.example")));
        assert!(guard.contains(&HeaderValue::from_static("https://b.example")));
        assert_eq!(guard.len(), 2);
    }

    #[test]
    fn seed_dedups() {
        let env = vec!["https://a.example".to_string()];
        let db = vec!["https://a.example".to_string()];
        let set = seed_origins(&env, &db);
        assert_eq!(set.read().unwrap().len(), 1);
    }

    #[test]
    fn seed_skips_blank_and_malformed() {
        let env = vec!["".to_string(), "  ".to_string(), "\u{0007}bad".to_string()];
        let db = vec!["https://ok.example".to_string()];
        let set = seed_origins(&env, &db);
        let guard = set.read().unwrap();
        assert_eq!(guard.len(), 1);
        assert!(guard.contains(&HeaderValue::from_static("https://ok.example")));
    }

    #[tokio::test]
    async fn middleware_preflight_allows_known_origin() {
        use tower::ServiceExt;
        let origins = seed_origins(&["https://a.example.com".to_string()], &[]);
        let svc = CorsLayer::new(origins).layer(tower::service_fn(
            |_req: axum::http::Request<axum::body::Body>| async {
                Ok::<_, std::convert::Infallible>(axum::response::Response::new(
                    axum::body::Body::empty(),
                ))
            },
        ));
        let req = axum::http::Request::builder()
            .method(Method::OPTIONS)
            .header(header::ORIGIN, "https://a.example.com")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers().get("access-control-allow-origin").unwrap(),
            "https://a.example.com"
        );
        assert!(resp.headers().get("access-control-allow-methods").is_some());
        assert!(resp.headers().get("access-control-allow-headers").is_some());
    }

    #[tokio::test]
    async fn middleware_preflight_blocks_unknown_origin() {
        use tower::ServiceExt;
        let origins = seed_origins(&[], &[]);
        let svc = CorsLayer::new(origins).layer(tower::service_fn(
            |_req: axum::http::Request<axum::body::Body>| async {
                Ok::<_, std::convert::Infallible>(axum::response::Response::new(
                    axum::body::Body::empty(),
                ))
            },
        ));
        let req = axum::http::Request::builder()
            .method(Method::OPTIONS)
            .header(header::ORIGIN, "https://evil.example.com")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        // Browser sees no ACAO → blocks the actual request itself.
        assert!(resp.headers().get("access-control-allow-origin").is_none());
    }

    #[tokio::test]
    async fn middleware_adds_acao_on_actual_request_when_allowed() {
        use tower::ServiceExt;
        let origins = seed_origins(&["https://a.example.com".to_string()], &[]);
        let svc = CorsLayer::new(origins).layer(tower::service_fn(
            |_req: axum::http::Request<axum::body::Body>| async {
                Ok::<_, std::convert::Infallible>(axum::response::Response::new(
                    axum::body::Body::empty(),
                ))
            },
        ));
        let req = axum::http::Request::builder()
            .method(Method::GET)
            .header(header::ORIGIN, "https://a.example.com")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get("access-control-allow-origin").unwrap(),
            "https://a.example.com"
        );
    }
}
