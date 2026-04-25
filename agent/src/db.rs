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
    // Phase 02: per-port opt-in for the local sites reverse proxy. Empty by default.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES ('proxy_allowed_ports', '[]')",
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
