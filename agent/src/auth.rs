use std::path::PathBuf;

use hmac::{Hmac, Mac};
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
