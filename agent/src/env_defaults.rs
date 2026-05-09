// Default runtime configuration for the agent binary.
//
// These values mirror `npm-wrapper/bin/oxiremote.js` DEFAULTS (lines 15-19),
// which already injects them for npm-installed users. Non-npm distributions
// (curl|bash, GitHub releases, source builds, `oxiremote update`) get the same
// behaviour via these in-binary defaults.
//
// Cross-reference: if any URL changes here it MUST also change in
// `npm-wrapper/bin/oxiremote.js` and vice versa.
//
// Resolution semantics (for each variable):
//   - env set and non-empty  → Some(env_value)   // env wins over default
//   - env set to empty       → None              // explicit opt-out
//   - env unset (VarError)   → Some(default)     // use bundled default
//
// No `unsafe`. Thread-safe via `OnceLock`.

use std::sync::OnceLock;

// Bundled defaults — kept in sync with npm-wrapper/bin/oxiremote.js:15-19.
pub const DEFAULT_DISCOVERY_URL: &str = "https://oxiremote-discovery.tienn.workers.dev";
pub const DEFAULT_WEB_URL: &str = "https://remote.erai.dev";
pub const DEFAULT_CORS_ORIGINS: &str = "https://remote.erai.dev";
pub const DEFAULT_SECURE_COOKIES: &str = "1";

// ── resolution logic ─────────────────────────────────────────────────────────

/// Read an env var and apply opt-out semantics.
///
/// - `Ok(s)` where `s` is non-empty → `Some(s)` (env wins)
/// - `Ok("")`                       → `None`    (explicit opt-out via empty value)
/// - `Err(_)` (unset)               → `Some(default.to_string())`
///
/// Available without a `OnceLock` cache so unit tests can call it in isolation
/// without cross-contamination from cached values.
pub fn resolve_uncached(key: &str, default: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(s) if !s.is_empty() => Some(s),
        Ok(_) => None,              // empty string = explicit opt-out
        Err(_) => Some(default.to_string()), // unset = use bundled default
    }
}

// ── typed public accessors ────────────────────────────────────────────────────
// Each accessor caches its result in a `OnceLock` so resolution runs exactly
// once per process lifetime (safe: config is immutable post-startup).

/// Cloudflare discovery worker base URL.
/// Returns `None` when the user explicitly cleared `OXI_DISCOVERY_URL=`.
/// `Some(DEFAULT_DISCOVERY_URL)` when the var is unset.
pub fn discovery_url() -> Option<String> {
    static LOCK: OnceLock<Option<String>> = OnceLock::new();
    LOCK.get_or_init(|| resolve_uncached("OXI_DISCOVERY_URL", DEFAULT_DISCOVERY_URL)).clone()
}

/// Public web app base URL (the user-facing SPA).
/// Returns `None` when the user explicitly cleared `OXI_WEB_URL=`.
/// `Some(DEFAULT_WEB_URL)` when the var is unset.
pub fn web_url() -> Option<String> {
    static LOCK: OnceLock<Option<String>> = OnceLock::new();
    LOCK.get_or_init(|| resolve_uncached("OXI_WEB_URL", DEFAULT_WEB_URL)).clone()
}

/// CORS origins string (comma-separated list).
/// Returns `None` when the user explicitly cleared `OXI_CORS_ORIGINS=`.
/// `Some(DEFAULT_CORS_ORIGINS)` when the var is unset.
pub fn cors_origins() -> Option<String> {
    static LOCK: OnceLock<Option<String>> = OnceLock::new();
    LOCK.get_or_init(|| resolve_uncached("OXI_CORS_ORIGINS", DEFAULT_CORS_ORIGINS)).clone()
}

/// Whether to mark auth cookies as `Secure`.
/// Parses "1" / "true" / "yes" (case-insensitive) as true.
/// Returns `false` when the user explicitly cleared `OXI_SECURE_COOKIES=` (opt-out).
/// Returns `true` (default "1") when the var is unset.
pub fn secure_cookies() -> bool {
    static LOCK: OnceLock<bool> = OnceLock::new();
    *LOCK.get_or_init(|| {
        match resolve_uncached("OXI_SECURE_COOKIES", DEFAULT_SECURE_COOKIES) {
            Some(s) => parse_bool_truthy(&s),
            None => false, // explicit empty = opt-out
        }
    })
}

/// Parse a string as a truthy boolean.
/// "1", "true", "yes" (any case) → true; everything else → false.
fn parse_bool_truthy(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower == "1" || lower == "true" || lower == "yes"
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Tests use `resolve_uncached` directly so they avoid the OnceLock cache.
    // We set env vars in each test — Rust test threads share the env so tests
    // that mutate the same key must not run concurrently. Use a unique key per
    // test or rely on `resolve_uncached` (which doesn't cache) instead of the
    // public accessors (which do cache via OnceLock).

    #[test]
    fn unset_returns_default() {
        // Use a key that is certainly not set in any real env.
        let sentinel = "__OXI_TEST_UNSET_KEY_THAT_DOES_NOT_EXIST__";
        // SAFETY: test-only; unique sentinel key not shared with other tests.
        unsafe { std::env::remove_var(sentinel) };
        let result = resolve_uncached(sentinel, DEFAULT_DISCOVERY_URL);
        assert_eq!(result, Some(DEFAULT_DISCOVERY_URL.to_string()));
    }

    #[test]
    fn set_value_returns_value() {
        let sentinel = "__OXI_TEST_SET_KEY_DISCOVERY__";
        // SAFETY: test-only; unique sentinel key not shared with other tests.
        unsafe { std::env::set_var(sentinel, "https://foo.dev") };
        let result = resolve_uncached(sentinel, DEFAULT_DISCOVERY_URL);
        unsafe { std::env::remove_var(sentinel) };
        assert_eq!(result, Some("https://foo.dev".to_string()));
    }

    #[test]
    fn empty_value_returns_none() {
        let sentinel = "__OXI_TEST_EMPTY_KEY__";
        // SAFETY: test-only; unique sentinel key not shared with other tests.
        unsafe { std::env::set_var(sentinel, "") };
        let result = resolve_uncached(sentinel, DEFAULT_DISCOVERY_URL);
        unsafe { std::env::remove_var(sentinel) };
        assert_eq!(result, None);
    }

    #[test]
    fn secure_cookies_parses_truthy() {
        assert!(parse_bool_truthy("1"));
        assert!(parse_bool_truthy("true"));
        assert!(parse_bool_truthy("yes"));
        assert!(parse_bool_truthy("TRUE"));
        assert!(parse_bool_truthy("True"));
        assert!(parse_bool_truthy("YES"));
    }

    #[test]
    fn secure_cookies_parses_falsy() {
        assert!(!parse_bool_truthy("0"));
        assert!(!parse_bool_truthy("false"));
        assert!(!parse_bool_truthy("no"));
        assert!(!parse_bool_truthy(""));
        assert!(!parse_bool_truthy("off"));
    }

    #[test]
    fn resolve_uncached_non_empty_env_wins_over_default() {
        let sentinel = "__OXI_TEST_WINS__";
        // SAFETY: test-only; unique sentinel key not shared with other tests.
        unsafe { std::env::set_var(sentinel, "https://self-hosted.example.com") };
        let result = resolve_uncached(sentinel, "https://should-not-appear.dev");
        unsafe { std::env::remove_var(sentinel) };
        assert_eq!(result, Some("https://self-hosted.example.com".to_string()));
    }

    #[test]
    fn default_secure_cookies_is_true() {
        // The bundled default is "1" which parses as true.
        assert!(parse_bool_truthy(DEFAULT_SECURE_COOKIES));
    }
}
