// Generic settings helpers — boolean key/value rows in the `settings` table.
// auto_approve has its own helpers in approval.rs (predates this module);
// new toggles should land here so we don't bloat per-feature modules.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

const KEY_DESKTOP_ENABLED: &str = "desktop_enabled";

fn get_bool(db_path: &PathBuf, key: &str, default: bool) -> bool {
    let Ok(conn) = Connection::open(db_path) else {
        return default;
    };
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![key],
            |r| r.get(0),
        )
        .ok();
    val.map(|v| v == "true").unwrap_or(default)
}

fn set_bool(db_path: &PathBuf, key: &str, enabled: bool) -> Result<()> {
    let conn = Connection::open(db_path).context("open db")?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, if enabled { "true" } else { "false" }],
    )
    .with_context(|| format!("upsert {key}"))?;
    Ok(())
}

pub fn get_desktop_enabled(db_path: &PathBuf) -> bool {
    get_bool(db_path, KEY_DESKTOP_ENABLED, true)
}

pub fn set_desktop_enabled(db_path: &PathBuf, enabled: bool) -> Result<()> {
    set_bool(db_path, KEY_DESKTOP_ENABLED, enabled)
}
