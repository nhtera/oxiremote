//! ICE server configuration shared by the agent's WebRTC peer connections
//! and the SPA (via `GET /api/hosts/{id}/desktop/capabilities`).
//!
//! Default is STUN-only (Google's public server), which requires a UDP
//! path between the browser and the host. Networks that block UDP or sit
//! behind symmetric NAT need a relay to carry WebRTC media at all — this
//! is how Chrome Remote Desktop stays usable on hotel/guest Wi-Fi. The
//! operator can point both sides at any TURN deployment (coturn,
//! Cloudflare Calls TURN, Twilio NTS, ...) with:
//!
//! - `OXI_STUN_URL` — override the default STUN server.
//! - `OXI_TURN_URL` — enable a TURN server (e.g. `turn:turn.example.com:3478`
//!   or `turns:turn.example.com:443?transport=tcp`).
//! - `OXI_TURN_USERNAME` / `OXI_TURN_PASSWORD` — long-term credentials for it.
//!
//! Unset TURN vars preserve today's STUN-only behaviour exactly. The
//! credential travels to paired devices only (the capabilities endpoint is
//! behind Bearer + cookie auth) — treat it like the API key it rides next
//! to, and prefer short-lived TURN credentials where available.

use serde::Serialize;

/// One ICE server entry in browser `RTCIceServer` shape. Serialized into
/// the capabilities response so the SPA passes it straight to
/// `new RTCPeerConnection({ iceServers })`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct IceServerEntry {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

pub const DEFAULT_STUN_URL: &str = "stun:stun.l.google.com:19302";

/// Assemble the ICE server list from optional operator overrides. Pure so
/// unit tests don't need to mutate process env.
fn build(
    stun_url: Option<String>,
    turn_url: Option<String>,
    turn_username: Option<String>,
    turn_password: Option<String>,
) -> Vec<IceServerEntry> {
    let mut servers = vec![IceServerEntry {
        urls: vec![stun_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_STUN_URL.to_string())],
        username: None,
        credential: None,
    }];
    if let Some(turn) = turn_url.filter(|s| !s.trim().is_empty()) {
        servers.push(IceServerEntry {
            urls: vec![turn],
            username: turn_username.filter(|s| !s.is_empty()),
            credential: turn_password.filter(|s| !s.is_empty()),
        });
    }
    servers
}

/// ICE servers per the `OXI_STUN_URL` / `OXI_TURN_*` env vars. Read on
/// each call — the values are only consulted at session/response setup,
/// never in a hot path.
pub fn from_env() -> Vec<IceServerEntry> {
    build(
        std::env::var("OXI_STUN_URL").ok(),
        std::env::var("OXI_TURN_URL").ok(),
        std::env::var("OXI_TURN_USERNAME").ok(),
        std::env::var("OXI_TURN_PASSWORD").ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_stun_only() {
        let servers = build(None, None, None, None);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, vec![DEFAULT_STUN_URL.to_string()]);
        assert!(servers[0].username.is_none());
        assert!(servers[0].credential.is_none());
    }

    #[test]
    fn stun_override_replaces_default() {
        let servers = build(Some("stun:stun.example.com:3478".into()), None, None, None);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, vec!["stun:stun.example.com:3478".to_string()]);
    }

    #[test]
    fn empty_stun_override_falls_back_to_default() {
        let servers = build(Some("  ".into()), None, None, None);
        assert_eq!(servers[0].urls, vec![DEFAULT_STUN_URL.to_string()]);
    }

    #[test]
    fn turn_appends_with_credentials() {
        let servers = build(
            None,
            Some("turns:turn.example.com:443?transport=tcp".into()),
            Some("user".into()),
            Some("pass".into()),
        );
        assert_eq!(servers.len(), 2);
        assert_eq!(
            servers[1].urls,
            vec!["turns:turn.example.com:443?transport=tcp".to_string()]
        );
        assert_eq!(servers[1].username.as_deref(), Some("user"));
        assert_eq!(servers[1].credential.as_deref(), Some("pass"));
    }

    #[test]
    fn credentials_are_omitted_from_json_when_absent() {
        let servers = build(None, None, None, None);
        let json = serde_json::to_string(&servers).unwrap();
        assert!(!json.contains("username"));
        assert!(!json.contains("credential"));
    }
}
