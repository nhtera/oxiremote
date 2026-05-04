// Web Push (RFC 8291 aes128gcm + RFC 8292 VAPID) — manual, rustls-friendly.
//
// Keeps the dep tree small: p256 for ECDH + ECDSA, aes-gcm for payload
// encryption, hkdf + sha2 for key derivation. JWT is hand-rolled (just
// base64url(header).base64url(payload).base64url(sig)).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use hkdf::Hkdf;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
    PublicKey, SecretKey,
};
use sha2::Sha256;

pub struct VapidKeys {
    pub secret: SecretKey,
    /// Uncompressed P-256 point (65 bytes), base64url no-pad. Shared with browsers.
    pub public_b64url: String,
}

impl VapidKeys {
    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from(&self.secret)
    }
}

pub fn load_or_create_vapid(data_dir: &Path) -> Result<VapidKeys> {
    let path = data_dir.join("vapid.key");
    let secret = if path.exists() {
        let bytes = std::fs::read(&path).context("read vapid.key")?;
        if bytes.len() != 32 {
            return Err(anyhow!("vapid.key: expected 32 bytes, got {}", bytes.len()));
        }
        SecretKey::from_slice(&bytes).context("parse vapid secret")?
    } else {
        let sk = SecretKey::random(&mut OsRng);
        write_secret(&path, &sk.to_bytes())?;
        sk
    };

    let encoded = secret.public_key().to_encoded_point(false);
    let public_b64url = URL_SAFE_NO_PAD.encode(encoded.as_bytes());

    Ok(VapidKeys { secret, public_b64url })
}

fn write_secret(path: &PathBuf, data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("create {}", path.display()))?;
        f.write_all(data).context("write vapid.key")?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data).context("write vapid.key")?;
    }
    Ok(())
}

/// Decode base64url (with or without padding) or standard base64.
fn decode_b64_loose(input: &str) -> Result<Vec<u8>> {
    if let Ok(v) = URL_SAFE_NO_PAD.decode(input) {
        return Ok(v);
    }
    if let Ok(v) = URL_SAFE.decode(input) {
        return Ok(v);
    }
    STANDARD.decode(input).context("decode base64")
}

/// Extract origin (`scheme://host[:port]`) from a push endpoint URL.
fn endpoint_origin(endpoint: &str) -> Result<String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| anyhow!("endpoint missing scheme"))?;
    let host_and_port = match rest.find('/') {
        Some(idx) => &rest[..idx],
        None => rest,
    };
    if host_and_port.is_empty() {
        return Err(anyhow!("endpoint missing host"));
    }
    Ok(format!("{scheme}://{host_and_port}"))
}

/// Build a VAPID JWT (ES256) for the given audience.
fn build_vapid_jwt(keys: &VapidKeys, audience: &str, subject: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock")?
        .as_secs();
    // 12 hours is the conservative ceiling per RFC 8292 §2 (must be ≤ 24h).
    let exp = now + 12 * 3600;

    let header = br#"{"typ":"JWT","alg":"ES256"}"#;
    let payload = serde_json::json!({
        "aud": audience,
        "exp": exp,
        "sub": subject,
    })
    .to_string();

    let header_b64 = URL_SAFE_NO_PAD.encode(header);
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let signing_input = format!("{header_b64}.{payload_b64}");

    let signing_key = keys.signing_key();
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

    Ok(format!("{signing_input}.{sig_b64}"))
}

/// Encrypt payload per RFC 8291 aes128gcm. Returns the full record body ready
/// to POST: `salt(16) || rs(4) || idlen(1) || as_pubkey(65) || ciphertext`.
fn encrypt_payload(p256dh_raw: &[u8], auth_secret: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    // RFC 8291 §2.1 mandates auth_secret is exactly 16 bytes.
    if auth_secret.len() != 16 {
        return Err(anyhow!("auth must be 16 bytes, got {}", auth_secret.len()));
    }
    let ua_pk = PublicKey::from_sec1_bytes(p256dh_raw).context("parse p256dh")?;

    // Application-server ephemeral keypair
    let as_sk = SecretKey::random(&mut OsRng);
    let as_pk_encoded = as_sk.public_key().to_encoded_point(false);
    let as_pk_bytes = as_pk_encoded.as_bytes(); // 65 bytes

    // ECDH
    let shared = p256::ecdh::diffie_hellman(as_sk.to_nonzero_scalar(), ua_pk.as_affine());
    let shared_bytes = shared.raw_secret_bytes();

    // Random 16-byte salt
    let mut salt = [0u8; 16];
    use p256::elliptic_curve::rand_core::RngCore;
    OsRng.fill_bytes(&mut salt);

    // Step 1: Extract IKM using auth_secret as salt, shared_secret as IKM.
    // Then Expand with key_info to 32 bytes.
    let hk_ikm = Hkdf::<Sha256>::new(Some(auth_secret), shared_bytes.as_ref());
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\x00");
    key_info.extend_from_slice(p256dh_raw);
    key_info.extend_from_slice(as_pk_bytes);
    let mut ikm = [0u8; 32];
    hk_ikm
        .expand(&key_info, &mut ikm)
        .map_err(|e| anyhow!("expand ikm: {e}"))?;

    // Step 2: Derive CEK (16 bytes) and NONCE (12 bytes) via HKDF(salt, IKM).
    let hk_prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    hk_prk
        .expand(b"Content-Encoding: aes128gcm\x00", &mut cek)
        .map_err(|e| anyhow!("expand cek: {e}"))?;
    let mut nonce_bytes = [0u8; 12];
    hk_prk
        .expand(b"Content-Encoding: nonce\x00", &mut nonce_bytes)
        .map_err(|e| anyhow!("expand nonce: {e}"))?;

    // Plaintext + padding delimiter (0x02 = last record) per RFC 8188.
    let mut plaintext = Vec::with_capacity(payload.len() + 1);
    plaintext.extend_from_slice(payload);
    plaintext.push(0x02);

    // Encrypt with AES-128-GCM.
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| anyhow!("cek length"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| anyhow!("aes-gcm encrypt: {e}"))?;

    // Assemble body: salt(16) || rs(4 BE) || idlen(1) || as_pub(65) || ciphertext
    let record_size: u32 = 4096;
    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&record_size.to_be_bytes());
    body.push(65);
    body.extend_from_slice(as_pk_bytes);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// Load or create the CLI notify token. The token lives at
/// `$data_dir/notify.token` with mode 0600 — only the local user (running the
/// `oxiremote notify` CLI) can read it, so browsers can't steal it via XSS.
pub fn load_or_create_notify_token(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("notify.token");
    if path.exists() {
        let raw = std::fs::read_to_string(&path).context("read notify.token")?;
        let token = raw.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let mut bytes = [0u8; 32];
    use p256::elliptic_curve::rand_core::RngCore;
    OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    write_secret(&path, token.as_bytes())?;
    Ok(token)
}

pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh_b64: String,
    pub auth_b64: String,
}

pub struct SendResult {
    pub status: u16,
    /// True when the subscription is permanently invalid (HTTP 404/410) and
    /// should be removed from the database.
    pub gone: bool,
}

/// Format a duration in milliseconds as a human-readable string like
/// "2m 30s" or "45s". Used in push notification bodies.
fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    }
}

pub struct AgentFinishedPushParams<'a> {
    pub host_id: &'a str,
    pub session_id: &'a str,
    pub agent_name: &'a str,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub tail_lines: Vec<String>,
}

/// Build and send a "agent finished" Web Push to all push subscriptions for
/// the given host. Called by the notifier when `SessionAgentEnded` fires and
/// the session has `notify_on_agent_end = true`.
///
/// Push payload shape:
///   title: `<agent_name> finished`
///   body:  `<duration> · exit <code> · <last 5 lines>`
///   tag:   `agent-end-<session_id>` (replaces prior push for same session)
///   data:  `{ url: "/h/<host_id>/workspace?session=<session_id>" }`
///
/// The body is hard-capped at 4 KB (web push limit).
pub async fn send_agent_finished_push(
    client: &reqwest::Client,
    keys: &VapidKeys,
    subscriptions: Vec<PushSubscription>,
    subject: &str,
    p: AgentFinishedPushParams<'_>,
) {
    if subscriptions.is_empty() {
        return;
    }

    let duration_str = format_duration(p.duration_ms);
    let exit_str = match p.exit_code {
        Some(c) => format!("exit {c}"),
        None => "exit ?".to_string(),
    };
    let lines_str = p.tail_lines.join(" \u{21b5} ");
    let body_raw = format!("{duration_str} \u{00b7} {exit_str} \u{00b7} {lines_str}");
    // Truncate body to 4 KB (conservative limit for FCM / Mozilla push).
    let body = if body_raw.len() > 4096 {
        let mut b = body_raw[..4096].to_string();
        // Avoid splitting a multi-byte UTF-8 codepoint.
        while !b.is_char_boundary(b.len()) {
            b.pop();
        }
        b
    } else {
        body_raw
    };

    let deep_link = format!("/h/{}/workspace?session={}", p.host_id, p.session_id);
    let tag = format!("agent-end-{}", p.session_id);

    let payload = serde_json::json!({
        "title": format!("{} finished", p.agent_name),
        "body": body,
        "tag": tag,
        "data": { "url": deep_link },
    });
    let payload_bytes = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(_) => return,
    };

    for sub in &subscriptions {
        match send_push(client, keys, sub, subject, &payload_bytes, 86400, "normal").await {
            Ok(r) if r.gone => {
                // Subscription is stale — caller should remove it. We cannot
                // do DB ops here without a db_path; log and move on.
                tracing::warn!(
                    endpoint = %sub.endpoint,
                    "push subscription gone (404/410) — should be pruned"
                );
            }
            Ok(r) if r.status >= 400 => {
                tracing::warn!(status = r.status, "agent-end push returned error status");
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(error=%err, "agent-end push send failed");
            }
        }
    }
}

pub async fn send_push(
    client: &reqwest::Client,
    keys: &VapidKeys,
    sub: &PushSubscription,
    subject: &str,
    payload: &[u8],
    ttl: u32,
    urgency: &str,
) -> Result<SendResult> {
    let p256dh = decode_b64_loose(&sub.p256dh_b64).context("decode p256dh")?;
    let auth = decode_b64_loose(&sub.auth_b64).context("decode auth")?;

    let body = encrypt_payload(&p256dh, &auth, payload)?;

    let audience = endpoint_origin(&sub.endpoint)?;
    let jwt = build_vapid_jwt(keys, &audience, subject)?;
    let auth_header = format!("vapid t={}, k={}", jwt, keys.public_b64url);

    let resp = client
        .post(&sub.endpoint)
        .header("Authorization", auth_header)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", ttl.to_string())
        .header("Urgency", urgency)
        .body(body)
        .send()
        .await
        .context("post to push endpoint")?;

    let status = resp.status().as_u16();
    Ok(SendResult { status, gone: matches!(status, 404 | 410) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_origin_extracts_scheme_host_port() {
        assert_eq!(
            endpoint_origin("https://fcm.googleapis.com/fcm/send/abc").unwrap(),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            endpoint_origin("https://updates.push.services.mozilla.com:443/wpush/v2/xyz").unwrap(),
            "https://updates.push.services.mozilla.com:443"
        );
        assert!(endpoint_origin("not-a-url").is_err());
    }

    #[test]
    fn load_or_create_vapid_persists() {
        let dir = std::env::temp_dir().join(format!("oxi-push-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let k1 = load_or_create_vapid(&dir).unwrap();
        let k2 = load_or_create_vapid(&dir).unwrap();
        assert_eq!(k1.public_b64url, k2.public_b64url);
        assert_eq!(k1.secret.to_bytes(), k2.secret.to_bytes());

        // Public key: 1 + 32 + 32 = 65 bytes uncompressed → 87 chars b64url no-pad.
        let pub_bytes = URL_SAFE_NO_PAD.decode(&k1.public_b64url).unwrap();
        assert_eq!(pub_bytes.len(), 65);
        assert_eq!(pub_bytes[0], 0x04);
    }

    #[test]
    fn jwt_has_three_parts() {
        let dir = std::env::temp_dir().join(format!("oxi-push-jwt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let k = load_or_create_vapid(&dir).unwrap();
        let jwt = build_vapid_jwt(&k, "https://fcm.googleapis.com", "mailto:test@local").unwrap();
        assert_eq!(jwt.matches('.').count(), 2);
    }

    #[test]
    fn encrypt_payload_shape() {
        // Generate a fake UA keypair to encrypt against.
        let ua_sk = SecretKey::random(&mut OsRng);
        let ua_pub_bytes = ua_sk.public_key().to_encoded_point(false).as_bytes().to_vec();
        let auth = [9u8; 16];

        let body = encrypt_payload(&ua_pub_bytes, &auth, b"hello").unwrap();

        // Minimum body = 16 (salt) + 4 (rs) + 1 (idlen) + 65 (pub) + 16 (GCM tag) + payload+1
        assert!(body.len() >= 16 + 4 + 1 + 65 + 16 + 6);
        assert_eq!(body[20], 65); // idlen
        // rs is big-endian 4096
        assert_eq!(&body[16..20], &4096u32.to_be_bytes());
    }
}
