// Simple token-bucket rate limiter keyed by (device_id, route_class).
//
// Keeps a DashMap of buckets. Each bucket tracks (tokens, last_refill_ts).
// Tokens refill continuously at `refill_per_sec` up to `capacity`. Consuming a
// token costs one; if none remain, the request is denied with 429.
//
// Route class is derived from the request path so we can apply stricter budgets
// to expensive endpoints (terminal WS open, file upload) separately from cheap
// reads.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;

use crate::db::now_ts;

use super::route_scope::is_tunnel_request;

#[derive(Clone, Debug)]
pub struct Bucket {
    pub tokens: f64,
    pub last_refill: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub capacity: f64,
    pub refill_per_sec: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouteClass {
    /// Cheap read endpoints (list/status/stat).
    Cheap,
    /// Writes and state-changing endpoints.
    Write,
    /// Expensive endpoints that open long-lived resources (terminal WS, uploads).
    Heavy,
}

impl RouteClass {
    pub fn budget(self) -> Budget {
        match self {
            // ~60 rpm with a 20-burst headroom — human-speed usage fits easily.
            RouteClass::Cheap => Budget { capacity: 20.0, refill_per_sec: 1.0 },
            // ~30 rpm with 10-burst; covers typical save/commit bursts.
            RouteClass::Write => Budget { capacity: 10.0, refill_per_sec: 0.5 },
            // 5-burst, replenished slowly — discourages scripted spam.
            RouteClass::Heavy => Budget { capacity: 5.0, refill_per_sec: 0.1 },
        }
    }
}

pub fn classify_route(path: &str, method: &axum::http::Method) -> RouteClass {
    if path.contains("/ws")
        || path.starts_with("/api/files/upload")
        || path.starts_with("/api/files/download")
        || path.starts_with("/preview/")
    {
        return RouteClass::Heavy;
    }
    match *method {
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS => {
            RouteClass::Cheap
        }
        _ => RouteClass::Write,
    }
}

pub type BucketKey = (String, RouteClass);

#[derive(Default)]
pub struct RateLimiter {
    buckets: DashMap<BucketKey, Bucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self { buckets: DashMap::new() }
    }

    /// Returns true if the request is allowed; false if it should be 429'd.
    pub fn allow(&self, device_id: &str, class: RouteClass) -> bool {
        let Budget { capacity, refill_per_sec } = class.budget();
        let now = now_ts();
        let key: BucketKey = (device_id.to_string(), class);
        let mut entry = self.buckets.entry(key).or_insert(Bucket {
            tokens: capacity,
            last_refill: now,
        });

        let elapsed = (now - entry.last_refill).max(0) as f64;
        entry.tokens = (entry.tokens + elapsed * refill_per_sec).min(capacity);
        entry.last_refill = now;

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Identify the caller for rate-limit bucketing. Prefers the session cookie;
/// falls back to the cf-connecting-ip for unauthenticated tunnel traffic so a
/// single hostile client still gets throttled.
fn caller_key(req: &Request) -> String {
    let headers = req.headers();
    if let Some(raw) = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok()) {
        for part in raw.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("oxiremote_session=")
                && let Some(sid) = rest.split('.').next() {
                    return format!("sess:{sid}");
                }
        }
    }
    if let Some(ip) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return format!("ip:{ip}");
    }
    "local".to_string()
}

pub async fn rate_limit(
    State(limiter): State<Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    // Only rate-limit tunnel traffic. Loopback callers (dev, CLI) are trusted.
    if !is_tunnel_request(req.headers()) {
        return next.run(req).await;
    }

    let class = classify_route(req.uri().path(), req.method());
    let key = caller_key(&req);

    if !limiter.allow(&key, class) {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("retry-after", "5")
            .body(Body::from("rate limited"))
            .unwrap();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_refills_and_drains() {
        let limiter = RateLimiter::new();
        let class = RouteClass::Cheap;
        let Budget { capacity, .. } = class.budget();
        // Drain the whole bucket.
        for _ in 0..(capacity as usize) {
            assert!(limiter.allow("dev-1", class));
        }
        // Next call without refill time should be denied.
        assert!(!limiter.allow("dev-1", class));
    }

    #[test]
    fn classify_heavy_routes() {
        assert_eq!(
            classify_route("/api/terminal/sessions/abc/ws", &axum::http::Method::GET),
            RouteClass::Heavy
        );
        assert_eq!(
            classify_route("/api/files/upload", &axum::http::Method::POST),
            RouteClass::Heavy
        );
        assert_eq!(
            classify_route("/preview/abc", &axum::http::Method::GET),
            RouteClass::Heavy
        );
    }

    #[test]
    fn classify_cheap_vs_write() {
        assert_eq!(
            classify_route("/api/files/list", &axum::http::Method::GET),
            RouteClass::Cheap
        );
        assert_eq!(
            classify_route("/api/files/write", &axum::http::Method::POST),
            RouteClass::Write
        );
    }
}
