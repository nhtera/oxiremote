// Generic settings helpers — boolean key/value rows in the `settings` table.
// auto_approve has its own helpers in approval.rs (predates this module);
// new toggles should land here so we don't bloat per-feature modules.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

const KEY_DESKTOP_ENABLED: &str = "desktop_enabled";
// Phase-02 scaffold. Capture path not implemented yet — readers will see
// `false` until phase-02 ships the cpal WASAPI pipeline.
const KEY_DESKTOP_AUDIO_ENABLED: &str = "desktop_audio_enabled";
// macOS lock-screen handling: while a desktop session is live, hold a
// `caffeinate` assertion + zero the screensaver idleTime so the host doesn't
// auto-lock. Default true on macOS (matches the user's expected
// out-of-the-box behaviour), false elsewhere (no-op platforms hide the
// toggle entirely via the capability snapshot).
const KEY_DESKTOP_STAY_AWAKE: &str = "desktop_stay_awake_during_session";

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

pub fn get_desktop_audio_enabled(db_path: &PathBuf) -> bool {
    get_bool(db_path, KEY_DESKTOP_AUDIO_ENABLED, false)
}

pub fn set_desktop_audio_enabled(db_path: &PathBuf, enabled: bool) -> Result<()> {
    set_bool(db_path, KEY_DESKTOP_AUDIO_ENABLED, enabled)
}

/// Whether to hold a stay-awake assertion for the desktop session lifetime.
/// Default true on macOS (where auto-lock turns the session into a black
/// frame the operator can't unlock remotely), false on Linux/Windows where
/// either the platform handles lock gracefully or stay-awake isn't wired up.
pub fn get_desktop_stay_awake(db_path: &PathBuf) -> bool {
    let default = cfg!(target_os = "macos");
    get_bool(db_path, KEY_DESKTOP_STAY_AWAKE, default)
}

pub fn set_desktop_stay_awake(db_path: &PathBuf, enabled: bool) -> Result<()> {
    set_bool(db_path, KEY_DESKTOP_STAY_AWAKE, enabled)
}
