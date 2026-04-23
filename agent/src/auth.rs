use std::path::PathBuf;

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
    let session_id = require_auth(signing_key, jar)?;
    let conn = Connection::open(db_path).ok()?;

    let row: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT s.device_id, d.revoked_at
             FROM sessions s
             LEFT JOIN trusted_devices d ON d.device_id = s.device_id
             WHERE s.session_id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match row {
        Some((Some(_device_id), revoked_at)) if revoked_at.is_none() => Some(session_id),
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

pub fn bind_session_to_device(db_path: &PathBuf, session_id: &str, device_id: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
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
}

pub fn list_trusted_devices(db_path: &PathBuf) -> anyhow::Result<Vec<TrustedDevice>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT device_id, label, user_agent, created_at, last_seen_at, revoked_at
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

pub fn is_valid_pairing_attempt(code: &str) -> bool {
    let trimmed = code.trim();
    trimmed.len() >= 6 && trimmed.len() <= 16
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

    use axum_extra::extract::cookie::{Cookie, CookieJar};
    use dashmap::DashMap;
    use rusqlite::{params, Connection};

    use super::*;
    use crate::{db::init_db, AppState};

    fn test_state(name: &str) -> AppState {
        let db_path = std::env::temp_dir().join(format!(
            "oxiremote-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&db_path);
        init_db(&db_path).unwrap();

        AppState {
            db_path,
            signing_key: b"01234567890123456789012345678901".to_vec(),
            secure_cookies: false,
            terminal_sessions: DashMap::new(),
            preview_targets: DashMap::new(),
            pairing_attempts: DashMap::new(),
            workspace_root: PathBuf::from("."),
            host_info: crate::host::HostInfo {
                host_id: "test-host-id".to_string(),
                label: "test-host".to_string(),
                platform: "test".to_string(),
            },
        }
    }

    #[test]
    fn pairing_input_validation_is_reasonable() {
        assert!(is_valid_pairing_attempt("ABC123"));
        assert!(!is_valid_pairing_attempt("A"));
        assert!(!is_valid_pairing_attempt("12345678901234567"));
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
