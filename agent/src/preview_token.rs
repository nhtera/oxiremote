use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::db::now_ts;

type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_SHARE_TTL_SECS: i64 = 30 * 60;

/// HMAC share token: `share.{host_id}.{preview_id}.{exp}.{hex-sig}`.
/// Distinct tag + shape from session tokens so verification cannot be confused.
pub fn sign_share_token(key: &[u8], host_id: &str, preview_id: &str, ttl_secs: i64) -> String {
    let exp = now_ts() + ttl_secs;
    let payload = format!("share.{host_id}.{preview_id}.{exp}");
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{payload}.{}", hex::encode(sig))
}

pub fn verify_share_token(key: &[u8], token: &str) -> Option<(String, String)> {
    let mut parts = token.splitn(5, '.');
    let tag = parts.next()?;
    if tag != "share" {
        return None;
    }
    let host_id = parts.next()?.to_string();
    let preview_id = parts.next()?.to_string();
    let exp_str = parts.next()?;
    let sig_hex = parts.next()?;

    let exp: i64 = exp_str.parse().ok()?;
    if now_ts() > exp {
        return None;
    }

    let payload = format!("share.{host_id}.{preview_id}.{exp}");
    let sig = hex::decode(sig_hex).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&sig).ok()?;

    Some((host_id, preview_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_roundtrip() {
        let key = b"01234567890123456789012345678901";
        let token = sign_share_token(key, "hostA", "prev123", 60);
        let parsed = verify_share_token(key, &token).unwrap();
        assert_eq!(parsed, ("hostA".to_string(), "prev123".to_string()));
    }

    #[test]
    fn rejects_tampered_token() {
        let key = b"01234567890123456789012345678901";
        let token = sign_share_token(key, "hostA", "prev123", 60);
        let tampered = token.replace("prev123", "prev999");
        assert!(verify_share_token(key, &tampered).is_none());
    }

    #[test]
    fn rejects_expired_token() {
        let key = b"01234567890123456789012345678901";
        let token = sign_share_token(key, "hostA", "prev123", -10);
        assert!(verify_share_token(key, &token).is_none());
    }

    #[test]
    fn rejects_session_token_shape() {
        // a session-like token lacks the "share" tag
        let fake = format!("session-1.{}.deadbeef", now_ts());
        let key = b"01234567890123456789012345678901";
        assert!(verify_share_token(key, &fake).is_none());
    }
}
