use std::path::Path;

use anyhow::Context;
use chrono::Utc;
use rusqlite::Connection;

pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

pub fn init_db(db_path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open(db_path).context("open db")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pairing_codes (
            code TEXT PRIMARY KEY,
            expires_at INTEGER NOT NULL,
            used_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            device_id TEXT
        );
        CREATE TABLE IF NOT EXISTS trusted_devices (
            device_id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            user_agent TEXT,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            revoked_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS terminal_sessions (
            terminal_session_id TEXT PRIMARY KEY,
            owner_session_id TEXT NOT NULL,
            name TEXT,
            cwd TEXT,
            command TEXT,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            cols INTEGER NOT NULL DEFAULT 80,
            rows INTEGER NOT NULL DEFAULT 24,
            status TEXT NOT NULL DEFAULT 'running',
            exit_code INTEGER
        );
        CREATE TABLE IF NOT EXISTS hosts (
            host_id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            platform TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS push_subscriptions (
            host_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            endpoint TEXT NOT NULL,
            p256dh TEXT NOT NULL,
            auth TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (host_id, device_id)
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            host_id TEXT NOT NULL,
            path TEXT NOT NULL,
            label TEXT NOT NULL,
            last_used_at INTEGER NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            UNIQUE(host_id, path)
        );
        CREATE INDEX IF NOT EXISTS idx_workspaces_host_last
            ON workspaces(host_id, last_used_at DESC);
        CREATE TABLE IF NOT EXISTS previews (
            id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            port INTEGER NOT NULL,
            label TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            UNIQUE(host_id, port)
        );
        CREATE INDEX IF NOT EXISTS idx_previews_host
            ON previews(host_id, created_at DESC);",
    )
    .context("create tables")?;

    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", []);
    // Index for device_id lookups on the sessions table (session-by-device queries).
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_device_id ON sessions(device_id)",
        [],
    );
    // Per-device API key for tunnel-side Bearer auth. Issued at pairing,
    // rotatable. `last4` is cosmetic — lets the UI show "••••abcd" to the
    // user without ever reading the hash.
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN api_key_hash TEXT", []);
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN api_key_last4 TEXT", []);

    // Migration 003: approval state on trusted_devices.
    // Default 'approved' so existing paired devices are unaffected.
    let _ = conn.execute(
        "ALTER TABLE trusted_devices ADD COLUMN approval_status TEXT NOT NULL DEFAULT 'approved'",
        [],
    );
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN first_seen_ip TEXT", []);
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN first_seen_ua TEXT", []);
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_devices_approval ON trusted_devices(approval_status)",
        [],
    );

    // Migration 004: one-time keys table.
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS one_time_keys (
            token TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER,
            issued_by_session TEXT
        )",
        [],
    );

    // Migration 005: settings table + seed rows.
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('auto_approve', 'false')",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('desktop_quality', 'med')",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('tunnel_mode', 'quick')",
        [],
    );
    // Operator can toggle remote desktop off so the WS upgrade returns 503
    // even when the binary was built with the desktop feature.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('desktop_enabled', 'true')",
        [],
    );
    // Per-port opt-in allowlist for the local-sites reverse proxy. Empty by default.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('proxy_allowed_ports', '[]')",
        [],
    );
    // Multi-host allowlist: SPA origins captured at pairing time so a tab on
    // host A's tunnel can cross-origin fetch host B's API. Quick Tunnel URLs
    // rotate, so this allowlist is dynamic — origins are added on every
    // successful pair and pruned via the localhost CORS API.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('cors_allowed_origins', '[]')",
        [],
    );

    // Permanent dashboard API key, seeded empty so the row always exists;
    // callers treat an empty hash as "not yet generated".
    //   permanent_key_hash       — Argon2id PHC string
    //   permanent_key_last4      — cosmetic last-4 for the "••••abcd" mask
    //   permanent_key_created_at — Unix timestamp of last rotation
    //   permanent_key_lookup_id  — sha256(plaintext)[..8] hex, registered with
    //                              the discovery worker so cross-origin SPAs
    //                              can resolve sk-… → tunnel_url. Non-secret.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('permanent_key_hash', '')",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('permanent_key_last4', '')",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('permanent_key_created_at', '0')",
        [],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('permanent_key_lookup_id', '')",
        [],
    );

    // Device identity columns for the TUI device manager and web dashboard.
    // NULL for pre-migration rows. `platform` is written at pairing time
    // (TUI/web fall back to parsing user_agent if NULL); `device_name` is
    // user-editable via PATCH /api/devices/{id}; `last_active_at` is touched
    // by api_key_guard on every Bearer request, debounced to 60s.
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN device_name TEXT", []);
    let _ = conn.execute("ALTER TABLE trusted_devices ADD COLUMN platform TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE trusted_devices ADD COLUMN last_active_at INTEGER",
        [],
    );

    // Stable agent identity for the discovery worker (Phase 02).
    // 32-byte random hex generated once on first boot — independent of
    // permanent_key_hash so a key rotation does not invalidate the existing
    // discovery session. The worker stores `discovery_id -> tunnelUrl`; the
    // SPA never sees this value.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('discovery_id', lower(hex(randomblob(32))))",
        [],
    );

    // Migration: agent-aware moat columns on terminal_sessions.
    // Whether the user opted in to "notify when agent finishes" for this
    // session (1 = enabled, 0 = disabled, default 0).
    let _ = conn.execute(
        "ALTER TABLE terminal_sessions ADD COLUMN notify_on_agent_end INTEGER DEFAULT 0",
        [],
    );
    // Name of the agent currently detected in this session (NULL when none).
    let _ = conn.execute(
        "ALTER TABLE terminal_sessions ADD COLUMN detected_agent TEXT",
        [],
    );

    // Migration: link consumed OTKs back to the device that consumed them so
    // Recent Key Usage can show "iPhone · 1.2.3.4" instead of "unknown" — the
    // legacy `issued_by_session` column points at the issuing dashboard
    // session, not the phone that scanned the QR.
    let _ = conn.execute(
        "ALTER TABLE one_time_keys ADD COLUMN consumed_by_device TEXT",
        [],
    );

    Ok(())
}

/// Read the persisted `/proxy/<port>/*` allowlist. Stored as a JSON array of
/// u16. Malformed values are treated as empty so a hand-edited DB never blocks
/// boot.
pub fn load_proxy_allowed_ports(db_path: &Path) -> anyhow::Result<Vec<u16>> {
    let conn = Connection::open(db_path).context("open db for proxy_allowed_ports load")?;
    let mut stmt =
        conn.prepare("SELECT value FROM settings WHERE key = 'proxy_allowed_ports'")?;
    let raw: Option<String> = stmt
        .query_row([], |row| row.get::<_, String>(0))
        .ok();
    let Some(text) = raw else { return Ok(Vec::new()) };
    let parsed: Vec<u16> = serde_json::from_str(&text).unwrap_or_default();
    Ok(parsed)
}

pub fn save_proxy_allowed_ports(db_path: &Path, ports: &[u16]) -> anyhow::Result<()> {
    let conn = Connection::open(db_path).context("open db for proxy_allowed_ports save")?;
    let value = serde_json::to_string(ports).context("encode ports json")?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('proxy_allowed_ports', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![value],
    )?;
    Ok(())
}

/// FIFO cap so a misbehaving client can't grow `cors_allowed_origins`
/// unboundedly. 50 entries covers any realistic deployment (≤5 hosts × a
/// handful of SPA origins each).
const CORS_ORIGINS_MAX: usize = 50;

/// Read the persisted CORS allowlist. Stored as a JSON array of origin strings
/// (e.g. `https://abc-123.trycloudflare.com`). Malformed JSON → empty list so
/// a hand-edited DB never blocks boot.
pub fn load_cors_allowed_origins(db_path: &Path) -> anyhow::Result<Vec<String>> {
    let conn = Connection::open(db_path).context("open db for cors_allowed_origins load")?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'cors_allowed_origins'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let Some(text) = raw else { return Ok(Vec::new()) };
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

/// Add `origin` to the allowlist (idempotent). FIFO-evicts the oldest entry
/// when the list reaches `CORS_ORIGINS_MAX`.
///
/// Concurrent pairs racing this function would TOCTOU — two readers see the
/// same baseline, each appends locally, last `UPDATE` wins, one origin is
/// lost. Wrapped in `BEGIN IMMEDIATE` so SQLite serializes the read-modify-
/// write under a write lock and races collapse to sequential.
pub fn upsert_cors_origin(db_path: &Path, origin: &str) -> anyhow::Result<()> {
    let mut conn = Connection::open(db_path).context("open db for cors_allowed_origins upsert")?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("begin immediate")?;
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = 'cors_allowed_origins'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let mut list: Vec<String> = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    if list.iter().any(|o| o == origin) {
        return Ok(());
    }
    if list.len() >= CORS_ORIGINS_MAX {
        list.remove(0);
    }
    list.push(origin.to_string());
    let value = serde_json::to_string(&list).context("encode cors origins json")?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES ('cors_allowed_origins', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![value],
    )?;
    tx.commit()?;
    Ok(())
}

/// Remove `origin` from the allowlist. No-op if missing.
pub fn remove_cors_origin(db_path: &Path, origin: &str) -> anyhow::Result<()> {
    let mut conn = Connection::open(db_path).context("open db for cors_allowed_origins remove")?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("begin immediate")?;
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = 'cors_allowed_origins'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let mut list: Vec<String> = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let before = list.len();
    list.retain(|o| o != origin);
    if list.len() == before {
        return Ok(());
    }
    let value = serde_json::to_string(&list).context("encode cors origins json")?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES ('cors_allowed_origins', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![value],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oxi-cors-test-{}-{}.db",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        init_db(&path).unwrap();
        path
    }

    #[test]
    fn upsert_cors_origin_is_idempotent() {
        let path = fresh_db("idempotent");
        upsert_cors_origin(&path, "https://a.example.com").unwrap();
        upsert_cors_origin(&path, "https://a.example.com").unwrap();
        let list = load_cors_allowed_origins(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], "https://a.example.com");
    }

    #[test]
    fn upsert_cors_origin_fifo_cap() {
        let path = fresh_db("fifo");
        for i in 0..(CORS_ORIGINS_MAX + 5) {
            upsert_cors_origin(&path, &format!("https://host{i}.example.com")).unwrap();
        }
        let list = load_cors_allowed_origins(&path).unwrap();
        assert_eq!(list.len(), CORS_ORIGINS_MAX);
        // Most recent must still be present; oldest evicted.
        let last = format!("https://host{}.example.com", CORS_ORIGINS_MAX + 4);
        assert!(list.contains(&last));
        assert!(!list.contains(&"https://host0.example.com".to_string()));
    }

    #[test]
    fn remove_cors_origin_removes_entry() {
        let path = fresh_db("remove");
        upsert_cors_origin(&path, "https://a.example.com").unwrap();
        upsert_cors_origin(&path, "https://b.example.com").unwrap();
        remove_cors_origin(&path, "https://a.example.com").unwrap();
        let list = load_cors_allowed_origins(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], "https://b.example.com");
    }

    #[test]
    fn remove_missing_origin_is_noop() {
        let path = fresh_db("noop");
        upsert_cors_origin(&path, "https://a.example.com").unwrap();
        remove_cors_origin(&path, "https://nope.example.com").unwrap();
        let list = load_cors_allowed_origins(&path).unwrap();
        assert_eq!(list.len(), 1);
    }

    /// Regression test for TOCTOU race in upsert_cors_origin. Without the
    /// IMMEDIATE transaction, two concurrent inserts of distinct origins
    /// would race: both read the same baseline, both append locally, one
    /// UPDATE wins, the other origin is dropped.
    #[test]
    fn upsert_concurrent_distinct_origins_all_persist() {
        let path = fresh_db("concurrent");
        let path_a = path.clone();
        let path_b = path.clone();
        let t1 = std::thread::spawn(move || {
            upsert_cors_origin(&path_a, "https://a.example.com").unwrap();
        });
        let t2 = std::thread::spawn(move || {
            upsert_cors_origin(&path_b, "https://b.example.com").unwrap();
        });
        t1.join().unwrap();
        t2.join().unwrap();
        let list = load_cors_allowed_origins(&path).unwrap();
        assert_eq!(list.len(), 2, "both origins must persist after race");
        assert!(list.contains(&"https://a.example.com".to_string()));
        assert!(list.contains(&"https://b.example.com".to_string()));
    }
}
