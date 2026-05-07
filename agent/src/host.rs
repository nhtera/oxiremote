use std::path::Path;

use anyhow::Context;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db::now_ts;

#[derive(Clone, Serialize)]
pub struct HostInfo {
    pub host_id: String,
    pub label: String,
    pub platform: String,
}

/// Derive a stable host_id: blake3(hostname XOR install_salt), hex-truncated to 16 chars.
///
/// We hash hostname + a per-install random salt so that:
/// - host_id doesn't leak the raw hostname over the wire
/// - two installs on the same machine get distinct IDs
pub fn ensure_host(data_dir: &Path, db: &Connection) -> anyhow::Result<HostInfo> {
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let salt = load_or_create_salt(data_dir)?;

    // XOR hostname bytes (repeated/truncated) with salt, then hash
    let hostname_bytes = hostname.as_bytes();
    let mut mixed = [0u8; 32];
    for (i, b) in salt.iter().enumerate() {
        mixed[i] = b ^ hostname_bytes[i % hostname_bytes.len()];
    }
    let hash = blake3::hash(&mixed);
    let host_id = hex::encode(&hash.as_bytes()[..8]); // 16 hex chars

    let label = hostname.clone();
    let platform = std::env::consts::OS.to_string();

    db.execute(
        "INSERT OR IGNORE INTO hosts(host_id, label, platform, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![host_id, label, platform, now_ts()],
    )
    .context("insert host row")?;

    Ok(HostInfo { host_id, label, platform })
}

fn load_or_create_salt(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join("host_salt");
    if path.exists() {
        // Self-heal: re-apply owner-only on every boot in case a legacy
        // install left wider perms. Failure is non-fatal here — host_salt is
        // a stable identity-derivation salt, not an authentication secret;
        // signing.key (auth.rs) carries the loud-failure SettingsHint hook.
        if let Err(err) = crate::secure_file::ensure_owner_only(&path) {
            tracing::warn!(error=%err, path=%path.display(), "could not enforce owner-only on host_salt");
        }
        let bytes = std::fs::read(&path).context("read host_salt")?;
        if bytes.len() >= 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes[..32]);
            return Ok(arr);
        }
    }

    // Generate 32 random bytes using rand (already in tree)
    let mut arr = [0u8; 32];
    use rand::RngCore;
    rand::rng().fill_bytes(&mut arr);

    crate::secure_file::write_secret(&path, &arr).context("write host_salt")?;
    Ok(arr)
}
