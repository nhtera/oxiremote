// Approval state machine for trusted devices.
// States: pending → approved | rejected.
// `auto_approve` is a setting, not a state — when enabled, new OTK-authenticated
// devices skip pending and land directly in approved.

use anyhow::Context;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;

use crate::db::now_ts;

#[derive(Debug, Clone, Serialize)]
pub struct PendingDevice {
    pub device_id: String,
    pub ip: String,
    pub ua_parsed: String,
    pub first_seen: i64,
}

/// Set a device's approval_status to 'approved'. Errors if no row matched.
pub fn approve_device(db_path: &PathBuf, device_id: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path).context("open db")?;
    let n = conn
        .execute(
            "UPDATE trusted_devices SET approval_status='approved' WHERE device_id=?1",
            params![device_id],
        )
        .context("approve device")?;
    if n == 0 {
        anyhow::bail!("device not found: {device_id}");
    }
    Ok(())
}

/// Set a device's approval_status to 'rejected'. Errors if no row matched.
pub fn reject_device(db_path: &PathBuf, device_id: &str) -> anyhow::Result<()> {
    let conn = Connection::open(db_path).context("open db")?;
    let n = conn
        .execute(
            "UPDATE trusted_devices SET approval_status='rejected' WHERE device_id=?1",
            params![device_id],
        )
        .context("reject device")?;
    if n == 0 {
        anyhow::bail!("device not found: {device_id}");
    }
    Ok(())
}

/// List all devices with approval_status = 'pending'.
pub fn list_pending(db_path: &PathBuf) -> anyhow::Result<Vec<PendingDevice>> {
    let conn = Connection::open(db_path).context("open db")?;
    let mut stmt = conn.prepare(
        "SELECT device_id,
                COALESCE(first_seen_ip, ''),
                COALESCE(first_seen_ua, ''),
                created_at
         FROM trusted_devices
         WHERE approval_status='pending' AND revoked_at IS NULL
         ORDER BY created_at DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(PendingDevice {
            device_id: row.get(0)?,
            ip: row.get(1)?,
            ua_parsed: row.get(2)?,
            first_seen: row.get(3)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Read the auto_approve setting from the settings table. Defaults to false.
pub fn get_auto_approve(db_path: &PathBuf) -> bool {
    let Ok(conn) = Connection::open(db_path) else {
        return false;
    };
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key='auto_approve'",
            [],
            |r| r.get(0),
        )
        .ok();
    val.map(|v| v == "true").unwrap_or(false)
}

/// Upsert the auto_approve setting.
pub fn set_auto_approve(db_path: &PathBuf, enabled: bool) -> anyhow::Result<()> {
    let conn = Connection::open(db_path).context("open db")?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('auto_approve', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![if enabled { "true" } else { "false" }],
    )
    .context("upsert auto_approve")?;
    Ok(())
}

/// Insert a new trusted device with the given approval_status, ip, ua, and
/// optional platform string.  Caller owns the connection — wrap in
/// `conn.transaction()` to participate in the multi-step pairing/OTK
/// atomicity flow (see http_pages.rs).
pub fn insert_device_with_approval_tx(
    conn: &Connection,
    device_id: &str,
    label: &str,
    user_agent: Option<&str>,
    ip: &str,
    approval_status: &str,
    platform: Option<&str>,
) -> anyhow::Result<()> {
    let now = now_ts();
    conn.execute(
        "INSERT INTO trusted_devices(
             device_id, label, user_agent, created_at, last_seen_at,
             revoked_at, approval_status, first_seen_ip, first_seen_ua, platform
         ) VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5, ?6, ?3, ?7)
         ON CONFLICT(device_id) DO UPDATE SET
             label=excluded.label,
             user_agent=excluded.user_agent,
             platform=COALESCE(excluded.platform, trusted_devices.platform),
             last_seen_at=excluded.last_seen_at",
        params![device_id, label, user_agent, now, approval_status, ip, platform],
    )
    .context("insert device with approval")?;
    Ok(())
}
