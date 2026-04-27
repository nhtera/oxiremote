// One-time key (OTK) generation, consumption, and expiry.
// Tokens are 80-bit random values encoded as 16-char lowercase base32 (RFC 4648).
// Consumption is atomic: a single UPDATE with rows_affected check prevents
// any TOCTOU race under concurrent requests.

use anyhow::{anyhow, Context};
use base32::Alphabet;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;

use crate::db::now_ts;

pub const OTK_TTL_SECS: i64 = 1800;

#[derive(Debug, Clone, Serialize)]
pub struct OtkRecord {
    pub token: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

/// Generates a fresh OTK, expires any prior live token for this agent, and
/// inserts the new record. Returns the new record.
pub fn generate_otk(db_path: &PathBuf, issued_by_session: Option<&str>) -> anyhow::Result<OtkRecord> {
    let now = now_ts();
    let bytes: [u8; 10] = rand::random();
    let token = base32::encode(Alphabet::Rfc4648 { padding: false }, &bytes).to_lowercase();
    let expires_at = now + OTK_TTL_SECS;

    let conn = Connection::open(db_path).context("open db")?;

    // Expire any existing live token (regen-invalidates-prior semantic).
    let _ = conn.execute(
        "UPDATE one_time_keys SET used_at=?1
         WHERE used_at IS NULL AND expires_at > ?1",
        params![now],
    );

    conn.execute(
        "INSERT INTO one_time_keys(token, created_at, expires_at, used_at, issued_by_session)
         VALUES (?1, ?2, ?3, NULL, ?4)",
        params![token, now, expires_at, issued_by_session],
    )
    .context("insert otk")?;

    Ok(OtkRecord {
        token,
        created_at: now,
        expires_at,
        used_at: None,
    })
}

/// Returns the currently-live OTK if one exists (unused, unexpired). Returns
/// `Ok(None)` cleanly when the table is empty, missing, or db file absent so
/// the TUI can render an empty pairing-key panel during startup races.
pub fn active_otk(db_path: &PathBuf) -> anyhow::Result<Option<OtkRecord>> {
    let now = now_ts();
    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let row = conn
        .query_row(
            "SELECT token, created_at, expires_at, used_at
             FROM one_time_keys
             WHERE used_at IS NULL AND expires_at > ?1
             ORDER BY created_at DESC LIMIT 1",
            params![now],
            |r| {
                Ok(OtkRecord {
                    token: r.get(0)?,
                    created_at: r.get(1)?,
                    expires_at: r.get(2)?,
                    used_at: r.get(3)?,
                })
            },
        )
        .optional()
        .unwrap_or(None);
    Ok(row)
}

/// Atomically consumes an OTK. Returns the record on success, or an error if
/// the token is unknown, already used, or expired. Caller owns the connection
/// — pass a `&Transaction` so a downstream insert/hash failure rolls back the
/// consume and the user can retry the same code.
pub fn consume_otk_tx(conn: &Connection, token: &str) -> anyhow::Result<OtkRecord> {
    let now = now_ts();
    let rows_affected = conn.execute(
        "UPDATE one_time_keys SET used_at=?1
         WHERE token=?2 AND used_at IS NULL AND expires_at > ?1",
        params![now, token],
    )?;

    if rows_affected != 1 {
        return Err(anyhow!("token not found, already used, or expired"));
    }

    let record = conn
        .query_row(
            "SELECT token, created_at, expires_at, used_at FROM one_time_keys WHERE token=?1",
            params![token],
            |row| {
                Ok(OtkRecord {
                    token: row.get(0)?,
                    created_at: row.get(1)?,
                    expires_at: row.get(2)?,
                    used_at: row.get(3)?,
                })
            },
        )
        .context("read consumed otk")?;

    Ok(record)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use rusqlite::Connection;

    use crate::db::{init_db, now_ts};

    use super::*;

    fn consume_otk_via_path(db_path: &PathBuf, token: &str) -> anyhow::Result<OtkRecord> {
        let conn = Connection::open(db_path).context("open db")?;
        consume_otk_tx(&conn, token)
    }

    fn temp_db(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oxiremote-otk-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&path);
        init_db(&path).unwrap();
        path
    }

    #[test]
    fn generate_returns_16_char_token() {
        let db = temp_db("gen");
        let rec = generate_otk(&db, None).unwrap();
        assert_eq!(rec.token.len(), 16, "token must be exactly 16 chars");
        assert!(rec.token.chars().all(|c| c.is_ascii_alphanumeric() && !c.is_uppercase()));
    }

    #[test]
    fn consume_succeeds_once_rejects_second() {
        let db = temp_db("consume");
        let rec = generate_otk(&db, None).unwrap();

        let first = consume_otk_via_path(&db, &rec.token);
        assert!(first.is_ok(), "first consume must succeed");

        let second = consume_otk_via_path(&db, &rec.token);
        assert!(second.is_err(), "second consume must fail");
    }

    #[test]
    fn consume_rejects_expired_token() {
        let db = temp_db("expired");
        let conn = Connection::open(&db).unwrap();
        let past = now_ts() - 100;
        conn.execute(
            "INSERT INTO one_time_keys(token, created_at, expires_at, used_at, issued_by_session)
             VALUES ('expiredtoken1234', ?1, ?2, NULL, NULL)",
            params![past, past + 1],
        )
        .unwrap();

        let result = consume_otk_via_path(&db, "expiredtoken1234");
        assert!(result.is_err(), "expired token must be rejected");
    }

    #[test]
    fn regen_expires_prior_token() {
        let db = temp_db("regen");
        let first = generate_otk(&db, None).unwrap();
        let _second = generate_otk(&db, None).unwrap();

        // First token should now be consumed (used_at set)
        let conn = Connection::open(&db).unwrap();
        let used_at: Option<i64> = conn
            .query_row(
                "SELECT used_at FROM one_time_keys WHERE token=?1",
                params![first.token],
                |r| r.get(0),
            )
            .unwrap();
        assert!(used_at.is_some(), "prior token must be marked used after regen");
    }

    #[test]
    fn atomic_consume_exactly_one_winner() {
        let db = Arc::new(temp_db("atomic"));
        let rec = generate_otk(&db, None).unwrap();
        let token = rec.token.clone();

        let barrier = Arc::new(Barrier::new(2));

        let db1 = db.clone();
        let tok1 = token.clone();
        let bar1 = barrier.clone();
        let t1 = std::thread::spawn(move || {
            bar1.wait(); // synchronize start
            consume_otk_via_path(&db1, &tok1).is_ok()
        });

        let db2 = db.clone();
        let tok2 = token.clone();
        let bar2 = barrier.clone();
        let t2 = std::thread::spawn(move || {
            bar2.wait(); // synchronize start
            consume_otk_via_path(&db2, &tok2).is_ok()
        });

        let r1 = t1.join().expect("thread 1 panicked");
        let r2 = t2.join().expect("thread 2 panicked");

        assert_eq!(
            r1 as u8 + r2 as u8,
            1,
            "exactly one thread must win the OTK consume race; r1={r1}, r2={r2}"
        );
    }
}
