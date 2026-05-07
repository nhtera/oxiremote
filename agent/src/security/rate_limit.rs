// Per-route rate limiter. Two layers:
//
// 1. **Per-rule pre-auth buckets** (Phase 06b / H7, H9). Specific paths like
//    `/api/pairing/exchange` and `/api/login/one-time` use fixed-window-with-
//    lockout buckets keyed by `(rule_id, ip)` — sessions and api_keys aren't
//    relevant pre-auth. A burst that exhausts the window triggers a longer
//    lockout so brute-force attackers can't just resume after the next refill.
//    The 429 response carries `Retry-After` in seconds so a SPA countdown UI
//    can show a sensible wait time.
//
// 2. **RouteClass token-bucket fallback** (existing). Authenticated tunnel
//    routes get token-bucket throttling keyed by `(session_or_ip, route_class)`
//    so cheap reads, writes, and heavy WS opens have separate budgets.
//
// Loopback callers (no `cf-connecting-ip` header) bypass both layers — TUI,
// dashboard, and CLI never hit the limiter.

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

/// Per-rule pre-auth bucket — fixed window with lockout on overflow.
///
/// Distinct from the token-bucket model used by `RouteClass`: pre-auth paths
/// have to be brute-force-hostile, so once `max_attempts` is hit inside
/// `window_secs`, the rule locks the IP out for `lockout_secs` even if the
/// window would otherwise have expired. This stops the trivial bypass where
/// an attacker times their next attempt to land just after the window.
#[derive(Debug)]
pub struct RateLimitRule {
    /// Stable identifier used in the bucket key. Must be unique across rules.
    pub id: &'static str,
    /// Path prefix this rule applies to. Matched as exact path or
    /// `<path_prefix>/...` so `/api/pairing/exchange` doesn't match
    /// `/api/pairing/exchange-foo`.
    pub path_prefix: &'static str,
    pub max_attempts: u32,
    pub window_secs: u64,
    pub lockout_secs: u64,
}

/// Pre-auth rules. `select_rule` returns the first match (`Iterator::find`),
/// so the rule prefixes MUST NOT overlap — adding `/api/login` alongside
/// `/api/login/one-time` would make the binding order-dependent and silently
/// shadow the more specific rule. The `select_rule_respects_segment_boundary`
/// test pins the current shape; extend it if you add overlapping prefixes
/// (and switch to longest-prefix-match while you're there).
pub const RULES: &[RateLimitRule] = &[
    // 5 attempts per 15 minutes per IP, then 15-minute lockout.
    // Matches the OWASP credential-stuffing baseline; pairing codes are
    // long-lived but should still be brute-force-hostile.
    RateLimitRule {
        id: "pairing-exchange",
        path_prefix: "/api/pairing/exchange",
        max_attempts: 5,
        window_secs: 900,
        lockout_secs: 900,
    },
    // 10 attempts per 5 minutes per IP, then 5-minute lockout.
    // OTKs are 16-char base32 (≈80 bits) — practically unbreakable, but a
    // dedicated bucket prevents an attacker from using OTK guesses to drain
    // the global limiter and DOS legitimate paired devices.
    RateLimitRule {
        id: "otk-login",
        path_prefix: "/api/login/one-time",
        max_attempts: 10,
        window_secs: 300,
        lockout_secs: 300,
    },
];

fn select_rule(path: &str) -> Option<&'static RateLimitRule> {
    RULES
        .iter()
        .find(|r| path == r.path_prefix || path.starts_with(&format!("{}/", r.path_prefix)))
}

/// Bucket key = (rule_id, ip). Stored as a flat string so it can share the
/// `DashMap` with `RouteClass` keys (different namespace prefix).
type RuleKey = String;

#[derive(Clone, Debug, Default)]
struct RuleBucket {
    attempts: u32,
    window_started: i64,
    locked_until: i64,
}

#[derive(Default)]
pub struct RateLimiter {
    buckets: DashMap<BucketKey, Bucket>,
    rule_buckets: DashMap<RuleKey, RuleBucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: DashMap::new(),
            rule_buckets: DashMap::new(),
        }
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

    /// Try to consume one attempt for `(rule, ip)`. On success returns `Ok(())`.
    /// On lockout / window-overflow returns `Err(retry_after_secs)` so the
    /// middleware can populate the `Retry-After` header.
    pub fn try_consume_rule(&self, rule: &RateLimitRule, ip: &str) -> Result<(), u64> {
        let now = now_ts();
        let key = format!("{}|{}", rule.id, ip);
        let mut entry = self.rule_buckets.entry(key).or_default();

        if now < entry.locked_until {
            return Err((entry.locked_until - now).max(1) as u64);
        }
        // Window expired — reset.
        if now - entry.window_started >= rule.window_secs as i64 {
            entry.attempts = 0;
            entry.window_started = now;
        }
        if entry.attempts >= rule.max_attempts {
            entry.locked_until = now + rule.lockout_secs as i64;
            return Err(rule.lockout_secs);
        }
        entry.attempts += 1;
        Ok(())
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
    if let Some(ip) = ip_from_headers(req.headers()) {
        return format!("ip:{ip}");
    }
    "local".to_string()
}

/// IP extracted from `cf-connecting-ip`. Returns None for loopback callers.
/// Used by per-rule buckets where session-keying is irrelevant (pre-auth).
fn ip_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|s| s.to_string())
}

fn rate_limited_response(retry_after_secs: u64) -> Response {
    Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header("retry-after", retry_after_secs.to_string())
        .body(Body::from("rate limited"))
        .unwrap()
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

    let path = req.uri().path();

    // Per-rule pre-auth path? Authoritative — does not fall through to the
    // RouteClass bucket. Without this short-circuit a brute-force attacker
    // could drain the per-rule bucket but keep firing under the more lenient
    // RouteClass::Write fallback.
    if let Some(rule) = select_rule(path) {
        let Some(ip) = ip_from_headers(req.headers()) else {
            return next.run(req).await;
        };
        if let Err(retry_after) = limiter.try_consume_rule(rule, &ip) {
            return rate_limited_response(retry_after);
        }
        return next.run(req).await;
    }

    let class = classify_route(path, req.method());
    let key = caller_key(&req);

    if !limiter.allow(&key, class) {
        return rate_limited_response(5);
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

    #[test]
    fn select_rule_matches_pairing_and_otk() {
        assert_eq!(
            select_rule("/api/pairing/exchange").map(|r| r.id),
            Some("pairing-exchange")
        );
        assert_eq!(
            select_rule("/api/login/one-time").map(|r| r.id),
            Some("otk-login")
        );
        // Non-matching paths return None so they fall through to RouteClass.
        assert!(select_rule("/api/health").is_none());
        assert!(select_rule("/api/pairing").is_none());
    }

    #[test]
    fn select_rule_respects_segment_boundary() {
        // `/api/pairing/exchange-foo` must not be classified as a pairing
        // request — the prefix needs a segment boundary.
        assert!(select_rule("/api/pairing/exchange-foo").is_none());
        assert!(select_rule("/api/login/one-time-tokens").is_none());
        // But sub-paths of the rule prefix DO match (forward-compat for
        // `/api/pairing/exchange/v2` etc).
        assert_eq!(
            select_rule("/api/pairing/exchange/anything").map(|r| r.id),
            Some("pairing-exchange")
        );
    }

    #[test]
    fn rule_allows_up_to_max_then_locks_out() {
        let limiter = RateLimiter::new();
        let rule = &RULES[0]; // pairing-exchange, 5 attempts
        for _ in 0..rule.max_attempts {
            assert!(limiter.try_consume_rule(rule, "1.2.3.4").is_ok());
        }
        // 6th attempt -> Err with retry_after >= lockout_secs (window also
        // exhausted, so the bucket transitions to locked_until = now + lockout).
        let err = limiter.try_consume_rule(rule, "1.2.3.4").unwrap_err();
        assert_eq!(err, rule.lockout_secs);
        // 7th attempt — still locked, retry_after should be > 0.
        let err2 = limiter.try_consume_rule(rule, "1.2.3.4").unwrap_err();
        assert!(err2 > 0);
    }

    #[test]
    fn rule_buckets_are_per_ip() {
        let limiter = RateLimiter::new();
        let rule = &RULES[0];
        for _ in 0..rule.max_attempts {
            assert!(limiter.try_consume_rule(rule, "1.2.3.4").is_ok());
        }
        assert!(limiter.try_consume_rule(rule, "1.2.3.4").is_err());
        // A different IP starts fresh.
        assert!(limiter.try_consume_rule(rule, "5.6.7.8").is_ok());
    }

    #[test]
    fn rule_buckets_are_per_rule() {
        let limiter = RateLimiter::new();
        let pairing = &RULES[0];
        let otk = &RULES[1];
        // Drain pairing.
        for _ in 0..pairing.max_attempts {
            assert!(limiter.try_consume_rule(pairing, "1.2.3.4").is_ok());
        }
        assert!(limiter.try_consume_rule(pairing, "1.2.3.4").is_err());
        // OTK from the same IP is unaffected.
        assert!(limiter.try_consume_rule(otk, "1.2.3.4").is_ok());
    }

    #[test]
    fn pairing_rule_blocks_after_5_regardless_of_payload_rotation() {
        // Real-world H7 scenario: attacker rotates pairing-code values across
        // attempts hoping the limiter is keyed on (ip, code). The bucket key
        // is (rule_id, ip) only — value rotation cannot bypass.
        let limiter = RateLimiter::new();
        let rule = &RULES[0];
        // Simulate 5 distinct codes — limiter sees only the rule + IP.
        for _ in 0..rule.max_attempts {
            assert!(limiter.try_consume_rule(rule, "1.2.3.4").is_ok());
        }
        // 6th attempt with a fresh "code" — still rejected.
        assert!(limiter.try_consume_rule(rule, "1.2.3.4").is_err());
    }
}
