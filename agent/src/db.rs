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
            last_seen_at INTEGER NOT NULL
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
        );",
    )
    .context("create tables")?;
    Ok(())
}
