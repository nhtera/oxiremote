use std::path::PathBuf;

use argon2::{
    password_hash::{
        rand_core::{OsRng, RngCore},
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
    Argon2,
};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use sha2::Sha256;

use crate::db::now_ts;

type HmacSha256 = Hmac<Sha256>;

pub const PAIRING_CODE_LEN: usize = 8;
pub const PAIRING_TTL_SECS: i64 = 5 * 60;
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

pub fn load_or_create_key(path: &PathBuf) -> anyhow::Result<Vec<u8>> {
    if path.exists() {
        return Ok(std::fs::read(path)?);
    }
    let key: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    std::fs::write(path, &key)?;
    Ok(key)
}

pub fn sign_session(signing_key: &[u8], session_id: &str) -> String {
    let issued_at = now_ts();
    let payload = format!("{session_id}.{issued_at}");

    let mut mac = HmacSha256::new_from_slice(signing_key).expect("hmac key");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();

    format!("{payload}.{}", hex::encode(sig))
}

pub fn verify_session(signing_key: &[u8], token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let session_id = parts.next()?.to_string();
    let issued_at_str = parts.next()?;
    let sig_hex = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let issued_at: i64 = issued_at_str.parse().ok()?;
    if now_ts() - issued_at > SESSION_TTL_SECS {
        return None;
    }

    let payload = format!("{session_id}.{issued_at}");
    let sig = hex::decode(sig_hex).ok()?;

    let mut mac = HmacSha256::new_from_slice(signing_key).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&sig).ok()?;

    Some(session_id)
}

pub fn new_pairing_code() -> String {
    use rand::{distr::Alphanumeric, Rng};
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(PAIRING_CODE_LEN)
        .map(char::from)
        .collect::<String>()
        .to_uppercase()
}

pub fn require_auth(signing_key: &[u8], jar: &axum_extra::extract::cookie::CookieJar) -> Option<String> {
    let cookie = jar.get("oxiremote_session")?;
    verify_session(signing_key, cookie.value())
}

pub fn require_active_auth(
    db_path: &PathBuf,
    signing_key: &[u8],
    jar: &axum_extra::extract::cookie::CookieJar,
) -> Option<String> {
    require_active_auth_with_device(db_path, signing_key, jar).map(|(s, _)| s)
}

/// Like `require_active_auth` but also returns the session's bound `device_id`.
/// Callers that authorise per-device resources (e.g. `/ws/desktop/{device_id}`)
/// must compare this against the caller-supplied identifier.
pub fn require_active_auth_with_device(
    db_path: &PathBuf,
    signing_key: &[u8],
    jar: &axum_extra::extract::cookie::CookieJar,
) -> Option<(String, String)> {
    let session_id = require_auth(signing_key, jar)?;
    let conn = Connection::open(db_path).ok()?;

    let row: Option<(Option<String>, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT s.device_id, d.revoked_at, d.approval_status
             FROM sessions s
             LEFT JOIN trusted_devices d ON d.device_id = s.device_id
             WHERE s.session_id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();

    match row {
        Some((Some(device_id), revoked_at, approval_status))
            if revoked_at.is_none()
                && approval_status.as_deref().unwrap_or("approved") == "approved" =>
        {
            Some((session_id, device_id))
        }
        _ => None,
    }
}

pub fn device_label_from_user_agent(user_agent: Option<&str>) -> String {
    let ua = user_agent.unwrap_or("Unknown device").trim();
    if ua.is_empty() {
        return "Unknown device".to_string();
    }

    let first = ua.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
    if first.is_empty() {
        "Unknown device".to_string()
    } else {
        first
    }
}

pub fn touch_session_and_device(db_path: &PathBuf, session_id: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    let now = now_ts();
    conn.execute(
        "UPDATE sessions SET last_seen_at=?2 WHERE session_id=?1",
        params![session_id, now],
    )?;
    conn.execute(
        "UPDATE trusted_devices
         SET last_seen_at=?2
         WHERE device_id = (SELECT device_id FROM sessions WHERE session_id=?1)",
        params![session_id, now],
    )?;
    Ok(())
}

pub fn revoke_device(db_path: &PathBuf, device_id: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    let now = now_ts();
    conn.execute(
        "UPDATE trusted_devices SET revoked_at=?2 WHERE device_id=?1 AND revoked_at IS NULL",
        params![device_id, now],
    )?;
    Ok(())
}

/// Rename a device. Sanitizes input: strips control chars, truncates to 64
/// chars, treats empty/whitespace as NULL (clears the name).
/// Only updates non-revoked devices.
pub fn rename_device(db_path: &PathBuf, device_id: &str, name: Option<&str>) -> anyhow::Result<()> {
    let sanitized: Option<String> = name.and_then(|s| {
        // Strip control characters (U+0000–U+001F and U+007F).
        let clean: String = s.chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .trim()
            .chars()
            .take(64)
            .collect();
        if clean.is_empty() { None } else { Some(clean) }
    });

    let conn = Connection::open(db_path)?;
    conn.execute(
        "UPDATE trusted_devices SET device_name=?2 WHERE device_id=?1 AND revoked_at IS NULL",
        params![device_id, sanitized],
    )?;
    Ok(())
}

/// Update `last_active_at` for a device with a 60-second debounce. SELECT
/// before UPDATE — skip if the stored value is within 60 000 ms of `now_ms`.
/// Intended to be called fire-and-forget; errors are silently dropped by the
/// caller.
pub fn touch_device_last_active(db_path: &PathBuf, device_id: &str, now_ms: i64) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;

    let last: Option<i64> = conn
        .query_row(
            "SELECT last_active_at FROM trusted_devices WHERE device_id=?1 AND revoked_at IS NULL",
            params![device_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    if let Some(ts) = last
        && now_ms - ts < 60_000
    {
        return Ok(());
    }

    conn.execute(
        "UPDATE trusted_devices SET last_active_at=?2 WHERE device_id=?1 AND revoked_at IS NULL",
        params![device_id, now_ms],
    )?;
    Ok(())
}

// Used in preview.rs integration tests which are cfg(test)-only.
#[allow(dead_code)]
pub fn insert_or_update_device(
    db_path: &PathBuf,
    device_id: &str,
    label: &str,
    user_agent: Option<&str>,
) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    let now = now_ts();
    conn.execute(
        "INSERT INTO trusted_devices(device_id, label, user_agent, created_at, last_seen_at, revoked_at)
         VALUES (?1, ?2, ?3, ?4, ?4, NULL)
         ON CONFLICT(device_id) DO UPDATE SET
           label=excluded.label,
           user_agent=excluded.user_agent,
           last_seen_at=excluded.last_seen_at,
           revoked_at=NULL",
        params![device_id, label, user_agent, now],
    )?;
    Ok(())
}

/// Bind a session row to a device. Caller owns the connection — pass a
/// `&Transaction` to participate in the pairing/OTK atomic flow.
pub fn bind_session_to_device_tx(conn: &Connection, session_id: &str, device_id: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE sessions SET device_id=?2 WHERE session_id=?1",
        params![session_id, device_id],
    )?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct TrustedDevice {
    pub device_id: String,
    pub label: String,
    pub user_agent: Option<String>,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub revoked_at: Option<i64>,
    /// NULL in pre-migration rows is surfaced as `None`; the SPA treats it as
    /// `'approved'` to preserve backward compatibility with legacy devices.
    pub approval_status: Option<String>,
    /// User-supplied display name. NULL until the user renames the device.
    pub device_name: Option<String>,
    /// Coarse OS/platform string captured at pairing time.
    pub platform: Option<String>,
    /// Unix-ms timestamp of the last authenticated request, debounced to 60s
    /// in `api_key_guard`. NULL until the device sends its first request.
    pub last_active_at: Option<i64>,
}

pub fn list_trusted_devices(db_path: &PathBuf) -> anyhow::Result<Vec<TrustedDevice>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT device_id, label, user_agent, created_at, last_seen_at, revoked_at,
                approval_status, device_name, platform, last_active_at
         FROM trusted_devices
         WHERE revoked_at IS NULL
         ORDER BY last_seen_at DESC, created_at DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(TrustedDevice {
            device_id: row.get(0)?,
            label: row.get(1)?,
            user_agent: row.get(2)?,
            created_at: row.get(3)?,
            last_seen_at: row.get(4)?,
            revoked_at: row.get(5)?,
            approval_status: row.get(6)?,
            device_name: row.get(7)?,
            platform: row.get(8)?,
            last_active_at: row.get(9)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn random_device_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn sanitize_device_label(label: Option<&str>, fallback_user_agent: Option<&str>) -> String {
    let provided = label.unwrap_or("").trim();
    if !provided.is_empty() {
        return provided.chars().take(80).collect();
    }
    device_label_from_user_agent(fallback_user_agent)
}

/// Generate + store a fresh API key for this device. Returns the plaintext
/// key (only time the caller sees it) + last4. Caller owns the connection —
/// pass a `&Transaction` so an Argon2/UPDATE failure rolls back the whole
/// pairing transaction and preserves the unconsumed pairing code for retry.
pub fn issue_api_key_tx(conn: &Connection, device_id: &str) -> anyhow::Result<(String, String)> {
    use base64::Engine as _;
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let last4: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(key.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {e}"))?
        .to_string();

    conn.execute(
        "UPDATE trusted_devices SET api_key_hash=?2, api_key_last4=?3 WHERE device_id=?1",
        params![device_id, hash, last4],
    )?;
    Ok((key, last4))
}

/// Return `Some(device_id)` if the presented key matches some device's hash.
/// Pre-filters by `api_key_last4` so the expensive Argon2 verify runs at most
/// once per request. Blocking; call from `spawn_blocking` in async contexts
/// (see `verify_api_key_async`).
pub fn verify_api_key(db_path: &PathBuf, presented: &str) -> Option<String> {
    if presented.len() < 4 {
        return None;
    }
    let last4: String = presented
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let conn = Connection::open(db_path).ok()?;
    let mut stmt = conn
        .prepare(
            // Defensive: enforce the approval gate for Bearer auth too, matching
            // the cookie path in `require_active_auth`. NULL is treated as approved
            // so pre-migration rows keep working.
            "SELECT device_id, api_key_hash FROM trusted_devices
             WHERE api_key_hash IS NOT NULL
               AND revoked_at IS NULL
               AND api_key_last4 = ?1
               AND (approval_status IS NULL OR approval_status = 'approved')",
        )
        .ok()?;
    let rows = stmt
        .query_map(params![last4], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()?;

    for row in rows.flatten() {
        let (device_id, hash) = row;
        if let Ok(parsed) = PasswordHash::new(&hash)
            && Argon2::default().verify_password(presented.as_bytes(), &parsed).is_ok() {
                return Some(device_id);
            }
    }
    None
}

/// Async wrapper: runs the Argon2 verify on the blocking thread pool so it
/// doesn't stall the Tokio runtime.
pub async fn verify_api_key_async(db_path: PathBuf, presented: String) -> Option<String> {
    tokio::task::spawn_blocking(move || verify_api_key(&db_path, &presented))
        .await
        .ok()
        .flatten()
}

/// Generate a new permanent dashboard API key, store the Argon2id hash in the
/// `settings` table, and return `(plaintext, last4, created_at)`.
/// The plaintext is returned exactly once — the caller must surface it to the
/// operator immediately because it is never stored in cleartext.
pub fn rotate_permanent_key(db_path: &PathBuf) -> anyhow::Result<(String, String, i64)> {
    use base64::Engine as _;
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    // URL-safe base64, no padding — matches the per-device key format.
    let key = format!(
        "sk-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    );
    let last4: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(key.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {e}"))?
        .to_string();

    let created_at = now_ts();
    let mut conn = Connection::open(db_path)?;
    // Wrap the three writes in a transaction so a concurrent rotation cannot
    // produce a hash/last4 mismatch (e.g. two browser tabs clicking Regenerate).
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES ('permanent_key_hash', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![hash],
    )?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES ('permanent_key_last4', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![last4],
    )?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES ('permanent_key_created_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![created_at.to_string()],
    )?;
    tx.commit()?;
    Ok((key, last4, created_at))
}

/// Read permanent key metadata (last4 + created_at) without ever exposing the
/// hash or plaintext. Returns `None` when no key has been generated yet (hash
/// is the empty seed row).
pub fn get_permanent_key_meta(db_path: &PathBuf) -> anyhow::Result<Option<(String, i64)>> {
    let conn = Connection::open(db_path)?;
    let hash: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'permanent_key_hash'",
            [],
            |row| row.get(0),
        )
        .ok();

    // Empty hash means the key has never been generated.
    if hash.as_deref().unwrap_or("").is_empty() {
        return Ok(None);
    }

    let last4: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'permanent_key_last4'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();

    let created_at: i64 = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'permanent_key_created_at'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    Ok(Some((last4, created_at)))
}

/// Verify the permanent dashboard key against the stored Argon2id hash.
/// Returns `true` when the presented key matches.  Blocking — call from
/// `spawn_blocking` in async contexts.
pub fn verify_permanent_key(db_path: &PathBuf, presented: &str) -> bool {
    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let hash: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'permanent_key_hash'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(h) = hash.filter(|s| !s.is_empty()) else {
        return false;
    };
    PasswordHash::new(&h)
        .map(|parsed| Argon2::default().verify_password(presented.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

pub fn is_valid_pairing_attempt(code: &str) -> bool {
    let trimmed = code.trim();
    trimmed.len() >= 6 && trimmed.len() <= 16
}

/// Derive a coarse platform string from a User-Agent header value.
/// Returns one of: "ios", "macos", "windows", "android", "linux", or None.
/// Matches the same heuristics used in the SPA's `parsePlatformBadge`.
pub fn platform_from_ua(ua: &str) -> Option<String> {
    let s = ua.to_lowercase();
    if s.contains("iphone") || s.contains("ipad") {
        Some("ios".into())
    } else if s.contains("android") {
        Some("android".into())
    } else if s.contains("mac") {
        Some("macos".into())
    } else if s.contains("windows") {
        Some("windows".into())
    } else if s.contains("linux") {
        Some("linux".into())
    } else {
        None
    }
}

pub fn client_ip_key(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "local".to_string())
}

pub fn now_bucket(seconds: i64) -> i64 {
    now_ts() / seconds
}

pub fn rate_limit_key(ip: &str, code: &str) -> String {
    format!("{ip}:{}", code.to_uppercase())
}

pub fn should_allow_pairing_attempt(
    attempts: &dashmap::DashMap<String, i64>,
    key: &str,
    limit: i64,
    window_secs: i64,
) -> bool {
    let bucket = now_bucket(window_secs);
    let composite = format!("{key}:{bucket}");
    let mut entry = attempts.entry(composite).or_insert(0);
    if *entry >= limit {
        return false;
    }
    *entry += 1;
    true
}

pub fn clear_stale_pairing_attempts(attempts: &dashmap::DashMap<String, i64>, window_secs: i64) {
    let current_bucket = now_bucket(window_secs);
    let stale_keys: Vec<String> = attempts
        .iter()
        .filter_map(|entry| {
            let key = entry.key();
            let bucket = key.rsplit(':').next()?.parse::<i64>().ok()?;
            if bucket < current_bucket {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect();

    for key in stale_keys {
        attempts.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum_extra::extract::cookie::{Cookie, CookieJar};
    use dashmap::DashMap;
    use rusqlite::{params, Connection};

    use super::*;
    use crate::{db::init_db, local_sites, AppState};

    fn test_state(name: &str) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "oxiremote-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&db_path);
        init_db(&db_path).unwrap();

        let data_dir = db_path.parent().unwrap().join(format!("oxi-data-{name}"));
        std::fs::create_dir_all(&data_dir).unwrap();

        AppState {
            db_path,
            signing_key: b"01234567890123456789012345678901".to_vec(),
            secure_cookies: false,
            terminal_sessions: Arc::new(DashMap::new()),
            preview_targets: DashMap::new(),
            preview_health: DashMap::new(),
            local_sites: local_sites::new_cache(),
            proxy_allowed_ports: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            pairing_attempts: DashMap::new(),
            workspace_root: PathBuf::from("."),
            host_info: crate::host::HostInfo {
                host_id: "test-host-id".to_string(),
                label: "test-host".to_string(),
                platform: "test".to_string(),
            },
            vapid_keys: std::sync::Arc::new(
                crate::push::load_or_create_vapid(&data_dir).unwrap(),
            ),
            notify_token: "test-token".to_string(),
            http_client: reqwest::Client::new(),
            preview_client: reqwest::Client::new(),
            rate_limiter: std::sync::Arc::new(crate::security::rate_limit::RateLimiter::new()),
            event_bus: crate::events::EventBus::new(),
            tunnel_url: std::sync::Arc::new(std::sync::RwLock::new(None)),
            latest_tunnel_step: std::sync::Arc::new(std::sync::RwLock::new(None)),
            recent_logs: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            desktop_available: false,
            desktop_service: None,
        }
    }

    #[test]
    fn pairing_input_validation_is_reasonable() {
        assert!(is_valid_pairing_attempt("ABC123"));
        assert!(!is_valid_pairing_attempt("A"));
        assert!(!is_valid_pairing_attempt("12345678901234567"));
    }

    #[test]
    fn list_trusted_devices_includes_approval_status() {
        let state = test_state("list-approval-status");
        let conn = Connection::open(&state.db_path).unwrap();
        let now = now_ts();

        // Insert three devices with distinct approval statuses.
        for (id, label, status) in [
            ("dev-approved", "Laptop", "approved"),
            ("dev-pending", "Phone", "pending"),
            ("dev-rejected", "Tablet", "rejected"),
        ] {
            conn.execute(
                "INSERT INTO trusted_devices(device_id, label, user_agent, created_at, last_seen_at, revoked_at, approval_status)
                 VALUES (?1, ?2, NULL, ?3, ?3, NULL, ?4)",
                params![id, label, now, status],
            )
            .unwrap();
        }

        let devices = list_trusted_devices(&state.db_path).unwrap();
        assert_eq!(devices.len(), 3);

        let find = |id: &str| {
            devices
                .iter()
                .find(|d| d.device_id == id)
                .expect("device not found")
                .approval_status
                .as_deref()
                .unwrap_or("")
                .to_string()
        };

        assert_eq!(find("dev-approved"), "approved");
        assert_eq!(find("dev-pending"), "pending");
        assert_eq!(find("dev-rejected"), "rejected");
    }

    #[test]
    fn rename_device_sanitizes_input() {
        let state = test_state("rename-sanitize");
        let conn = Connection::open(&state.db_path).unwrap();
        let now = now_ts();

        conn.execute(
            "INSERT INTO trusted_devices(device_id, label, user_agent, created_at, last_seen_at, revoked_at)
             VALUES (?1, ?2, NULL, ?3, ?3, NULL)",
            params!["dev-rename", "Old Name", now],
        ).unwrap();

        // Empty string → NULL (clears name).
        rename_device(&state.db_path, "dev-rename", Some("")).unwrap();
        let name: Option<String> = conn
            .query_row("SELECT device_name FROM trusted_devices WHERE device_id='dev-rename'", [], |r| r.get(0))
            .unwrap();
        assert!(name.is_none(), "empty string should produce NULL device_name");

        // Normal name stored as-is (up to 64 chars).
        rename_device(&state.db_path, "dev-rename", Some("My Phone")).unwrap();
        let name: Option<String> = conn
            .query_row("SELECT device_name FROM trusted_devices WHERE device_id='dev-rename'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name.as_deref(), Some("My Phone"));

        // 65-char input truncated to 64.
        let long_name = "A".repeat(65);
        rename_device(&state.db_path, "dev-rename", Some(&long_name)).unwrap();
        let name: Option<String> = conn
            .query_row("SELECT device_name FROM trusted_devices WHERE device_id='dev-rename'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name.as_deref().map(|s| s.len()), Some(64));

        // Control chars stripped.
        rename_device(&state.db_path, "dev-rename", Some("Hello\x01\x1fWorld")).unwrap();
        let name: Option<String> = conn
            .query_row("SELECT device_name FROM trusted_devices WHERE device_id='dev-rename'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name.as_deref(), Some("HelloWorld"));
    }

    #[test]
    fn active_auth_rejects_revoked_device() {
        let state = test_state("active-auth");
        let conn = Connection::open(&state.db_path).unwrap();
        let now = now_ts();

        conn.execute(
            "INSERT INTO trusted_devices(device_id, label, user_agent, created_at, last_seen_at, revoked_at)
             VALUES (?1, ?2, NULL, ?3, ?3, NULL)",
            params!["device-1", "Phone", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params!["session-1", now, "device-1"],
        )
        .unwrap();

        let token = sign_session(&state.signing_key, "session-1");
        let jar = CookieJar::new().add(Cookie::new("oxiremote_session", token));

        assert_eq!(
            require_active_auth(&state.db_path, &state.signing_key, &jar),
            Some("session-1".to_string())
        );

        revoke_device(&state.db_path, "device-1").unwrap();

        assert_eq!(require_active_auth(&state.db_path, &state.signing_key, &jar), None);
    }
}
