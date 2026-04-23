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
            ON workspaces(host_id, last_used_at DESC);",
    )
    .context("create tables")?;

    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", []);

    Ok(())
}
