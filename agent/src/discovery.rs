//! Cloudflare discovery-worker client.
//!
//! Maps the agent's stable `discovery_id` to its current quick-tunnel URL so a
//! standalone SPA (Cloudflare Pages) can resolve us from a short-lived `tempKey`
//! embedded in a QR code. No-op when `OXI_DISCOVERY_URL` is unset — single-
//! binary embedded mode is unaffected.
//!
//! Wire: server_main subscribes to the event bus; on every `TunnelUrlChanged`
//! it calls `spawn_register`. Three sequential POSTs (create -> update ->
//! temp-key) with bounded retry; on success the temp key lands in
//! `AppState::discovery_temp_key` and the TUI/SPA pick it up on next draw.

use std::path::Path;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::events::{AgentEvent, EventBus};

/// Worker-side TTL on the temp key (matches `phase-01-discovery-worker.md`).
pub const TEMP_KEY_EXPIRY_MINUTES: u32 = 30;

const RETRY_ATTEMPTS: u32 = 3;
const RETRY_BASE_MS: u64 = 2_000;
const JITTER_MAX_MS: u64 = 1_000;

#[derive(Serialize)]
struct SessionCreateBody<'a> {
    #[serde(rename = "apiKey")]
    api_key: &'a str,
}

#[derive(Serialize)]
struct SessionUpdateBody<'a> {
    #[serde(rename = "apiKey")]
    api_key: &'a str,
    #[serde(rename = "tunnelUrl")]
    tunnel_url: &'a str,
}

#[derive(Serialize)]
struct TempKeyBody<'a> {
    #[serde(rename = "apiKey")]
    api_key: &'a str,
    #[serde(rename = "expiryMinutes")]
    expiry_minutes: u32,
}

#[derive(Serialize)]
struct CodeRegisterBody<'a> {
    #[serde(rename = "apiKey")]
    api_key: &'a str,
    code: &'a str,
    #[serde(rename = "expiryMinutes")]
    expiry_minutes: u32,
}

#[derive(Deserialize)]
struct TempKeyResponse {
    #[serde(rename = "tempKey")]
    temp_key: String,
}

/// Read the agent's stable discovery identity from the `settings` table.
/// Seeded by `db::init_db` on first boot; survives key rotations.
pub fn load_discovery_id(db_path: &Path) -> Result<String> {
    let conn = rusqlite::Connection::open(db_path).context("open db for discovery_id")?;
    let val: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'discovery_id'",
            [],
            |r| r.get(0),
        )
        .context("discovery_id row missing — db::init_db must run before this")?;
    if val.is_empty() {
        return Err(anyhow!("discovery_id is empty"));
    }
    Ok(val)
}

/// Latest unused, unexpired pairing code paired with its remaining lifetime
/// (minutes, rounded up, clamped to >= 1). Returns None when no valid code is
/// available — fresh boots always have one but a 5-minute-old code that has
/// already lapsed should not be re-registered.
pub fn active_pairing_code(db_path: &Path) -> Result<Option<(String, u32)>> {
    let now = crate::db::now_ts();
    let conn = rusqlite::Connection::open(db_path).context("open db for pairing_code")?;
    let row: rusqlite::Result<(String, i64)> = conn.query_row(
        "SELECT code, expires_at FROM pairing_codes
         WHERE used_at IS NULL AND expires_at > ?1
         ORDER BY expires_at DESC LIMIT 1",
        [now],
        |r| Ok((r.get(0)?, r.get(1)?)),
    );
    match row {
        Ok((code, expires_at)) => {
            let secs_left = (expires_at - now).max(0);
            let mins = ((secs_left + 59) / 60).max(1) as u32;
            Ok(Some((code, mins)))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(anyhow!(e)),
    }
}

/// Spawn a non-blocking task that registers the agent's session with the
/// worker and writes the issued temp key into `temp_key_slot`. Retries with
/// exponential backoff + jitter; emits `DiscoveryTempKeyIssued` on success or
/// `DiscoveryUnavailable` after all retries are exhausted.
///
/// `pairing_code` is optionally registered with the worker after the session
/// is established so the SPA can resolve `?code=ABCD1234` → tunnelUrl for
/// the manual-entry flow. Failure here is non-fatal — QR-scan still works.
pub fn spawn_register(
    client: Client,
    discovery_url: String,
    discovery_id: String,
    tunnel_url: String,
    temp_key_slot: Arc<StdRwLock<Option<String>>>,
    event_bus: Arc<EventBus>,
    pairing_code: Option<(String, u32)>,
) {
    tokio::spawn(async move {
        match register_with_retry(&client, &discovery_url, &discovery_id, &tunnel_url).await {
            Ok(temp_key) => {
                let prefix: String = temp_key.chars().take(4).collect();
                if let Ok(mut g) = temp_key_slot.write() {
                    *g = Some(temp_key);
                }
                info!(prefix = %prefix, "discovery temp key issued");
                event_bus.send(AgentEvent::DiscoveryTempKeyIssued { key_prefix: prefix });

                if let Some((code, mins)) = pairing_code {
                    match register_code(&client, &discovery_url, &discovery_id, &code, mins).await {
                        Ok(()) => debug!(mins, "pairing code registered with discovery worker"),
                        Err(e) => warn!(error = %e, "code/register failed (manual code-entry will fall back to QR)"),
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "discovery registration failed after retries");
                event_bus.send(AgentEvent::DiscoveryUnavailable);
            }
        }
    });
}

/// Fire-and-forget code registration. Use when an OTK or pairing code is
/// minted on a code path that already runs inside the tokio runtime — keeps
/// callers free of async error handling for a non-critical side effect.
pub fn spawn_register_code(
    client: Client,
    discovery_url: String,
    discovery_id: String,
    code: String,
    expiry_minutes: u32,
) {
    tokio::spawn(async move {
        match register_code(&client, &discovery_url, &discovery_id, &code, expiry_minutes).await {
            Ok(()) => debug!(expiry_minutes, "lookup code registered with discovery worker"),
            Err(e) => warn!(error = %e, "discovery code/register failed (cross-origin manual entry will fail until next rotation)"),
        }
    });
}

/// Register a user-typed pairing code with the worker so the cross-origin
/// SPA can resolve it without needing the QR's temp_key. Single attempt,
/// best-effort: agents always rebroadcast on TunnelUrlChanged anyway.
async fn register_code(
    client: &Client,
    base: &str,
    discovery_id: &str,
    code: &str,
    expiry_minutes: u32,
) -> Result<()> {
    let base = base.trim_end_matches('/');
    let res = client
        .post(format!("{base}/api/code/register"))
        .json(&CodeRegisterBody {
            api_key: discovery_id,
            code,
            expiry_minutes,
        })
        .send()
        .await
        .context("code/register POST")?;
    if !res.status().is_success() {
        return Err(anyhow!("code/register -> {}", res.status()));
    }
    Ok(())
}

async fn register_with_retry(
    client: &Client,
    base: &str,
    discovery_id: &str,
    tunnel_url: &str,
) -> Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=RETRY_ATTEMPTS {
        match register_session(client, base, discovery_id, tunnel_url).await {
            Ok(temp_key) => return Ok(temp_key),
            Err(err) => {
                debug!(attempt, error = %err, "discovery attempt failed");
                last_err = Some(err);
                if attempt < RETRY_ATTEMPTS {
                    let backoff = RETRY_BASE_MS << (attempt - 1);
                    let jitter = rand::rng().random_range(0..JITTER_MAX_MS);
                    tokio::time::sleep(Duration::from_millis(backoff + jitter)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("discovery failed (no error captured)")))
}

/// Cloudflare tunnels (quick + named) terminate HTTPS, but the published URL
/// shape differs: quick tunnel emits `https://abc.trycloudflare.com`, named
/// tunnel emits the bare hostname `oxiremote.example.com`. The SPA does
/// `${tunnelUrl}/api/...` and only works with full URLs, so normalize before
/// the worker sees the value.
fn normalize_tunnel_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

async fn register_session(
    client: &Client,
    base: &str,
    discovery_id: &str,
    tunnel_url: &str,
) -> Result<String> {
    let base = base.trim_end_matches('/');
    let normalized = normalize_tunnel_url(tunnel_url);
    let tunnel_url = normalized.as_str();

    // 1) session/create — idempotent upsert.
    let res = client
        .post(format!("{base}/api/session/create"))
        .json(&SessionCreateBody { api_key: discovery_id })
        .send()
        .await
        .context("session/create POST")?;
    if !res.status().is_success() {
        return Err(anyhow!("session/create -> {}", res.status()));
    }

    // 2) session/update — write the current tunnel URL.
    let res = client
        .post(format!("{base}/api/session/update"))
        .json(&SessionUpdateBody {
            api_key: discovery_id,
            tunnel_url,
        })
        .send()
        .await
        .context("session/update POST")?;
    if !res.status().is_success() {
        return Err(anyhow!("session/update -> {}", res.status()));
    }

    // 3) temp-key/create — mint a fresh 30-minute temp key.
    let res = client
        .post(format!("{base}/api/temp-key/create"))
        .json(&TempKeyBody {
            api_key: discovery_id,
            expiry_minutes: TEMP_KEY_EXPIRY_MINUTES,
        })
        .send()
        .await
        .context("temp-key/create POST")?;
    if !res.status().is_success() {
        return Err(anyhow!("temp-key/create -> {}", res.status()));
    }
    let body: TempKeyResponse = res
        .json()
        .await
        .context("parse temp-key/create response")?;
    if body.temp_key.is_empty() {
        return Err(anyhow!("temp-key/create returned empty tempKey"));
    }
    Ok(body.temp_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Mutex;

    use axum::Router;
    use axum::extract::State as AxumState;
    use axum::routing::post;
    use axum::{Json, http::StatusCode};
    use serde_json::Value;

    #[derive(Clone, Default)]
    struct MockState {
        bodies: Arc<Mutex<Vec<(String, Value)>>>,
        // Hits-to-fail; e.g. [true, true, false] => fail twice then succeed.
        fail_seq: Arc<Mutex<Vec<bool>>>,
    }

    async fn record(
        AxumState(state): AxumState<MockState>,
        path: &'static str,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        state.bodies.lock().unwrap().push((path.to_string(), body));
        let fail = {
            let mut seq = state.fail_seq.lock().unwrap();
            if seq.is_empty() { false } else { seq.remove(0) }
        };
        if fail {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        match path {
            "/api/temp-key/create" => Ok(Json(serde_json::json!({
                "tempKey": "deadbeefcafe1234deadbeefcafe1234",
                "expiresAt": 0
            }))),
            _ => Ok(Json(serde_json::json!({"ok": true}))),
        }
    }

    fn router(state: MockState) -> Router {
        Router::new()
            .route(
                "/api/session/create",
                post(|s, b| record(s, "/api/session/create", b)),
            )
            .route(
                "/api/session/update",
                post(|s, b| record(s, "/api/session/update", b)),
            )
            .route(
                "/api/temp-key/create",
                post(|s, b| record(s, "/api/temp-key/create", b)),
            )
            .route(
                "/api/code/register",
                post(|s, b| record(s, "/api/code/register", b)),
            )
            .with_state(state)
    }

    async fn spawn_mock(state: MockState) -> String {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let bound = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router(state)).await;
        });
        format!("http://{bound}")
    }

    #[test]
    fn normalize_tunnel_url_adds_scheme_when_missing() {
        assert_eq!(
            normalize_tunnel_url("oxiremote.erai.dev"),
            "https://oxiremote.erai.dev"
        );
        assert_eq!(
            normalize_tunnel_url("https://abc.trycloudflare.com"),
            "https://abc.trycloudflare.com"
        );
        assert_eq!(
            normalize_tunnel_url("http://localhost:8787"),
            "http://localhost:8787"
        );
        assert_eq!(
            normalize_tunnel_url("  oxiremote.erai.dev  "),
            "https://oxiremote.erai.dev"
        );
    }

    #[tokio::test]
    async fn register_session_normalizes_bare_hostname() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        register_session(&client, &url, "id", "oxiremote.erai.dev").await.unwrap();

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies[1].1["tunnelUrl"], "https://oxiremote.erai.dev");
    }

    #[tokio::test]
    async fn round_trip_posts_three_routes_in_order() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        let temp_key = register_session(&client, &url, "deadbeef", "https://t.example").await.unwrap();
        assert_eq!(temp_key, "deadbeefcafe1234deadbeefcafe1234");

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0].0, "/api/session/create");
        assert_eq!(bodies[0].1["apiKey"], "deadbeef");
        assert_eq!(bodies[1].0, "/api/session/update");
        assert_eq!(bodies[1].1["tunnelUrl"], "https://t.example");
        assert_eq!(bodies[2].0, "/api/temp-key/create");
        assert_eq!(bodies[2].1["expiryMinutes"], 30);
    }

    #[tokio::test]
    async fn retry_recovers_after_two_failures() {
        let mock = MockState::default();
        // First two register_session attempts fail on session/create (return 500
        // then short-circuit retry), third succeeds. Each attempt makes 3 POSTs
        // when successful, 1 POST when the first fails — sequence below.
        *mock.fail_seq.lock().unwrap() = vec![true, true, false, false, false];
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        let temp_key = register_with_retry(&client, &url, "id", "https://t").await.unwrap();
        assert_eq!(temp_key, "deadbeefcafe1234deadbeefcafe1234");
    }

    #[tokio::test]
    async fn all_retries_exhausted_returns_error() {
        let mock = MockState::default();
        *mock.fail_seq.lock().unwrap() = vec![true; 16];
        let url = spawn_mock(mock).await;
        let client = Client::new();

        let result = register_with_retry(&client, &url, "id", "https://t").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_code_posts_to_code_register_endpoint() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        register_code(&client, &url, "deadbeef", "ABCD1234", 5).await.unwrap();

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].0, "/api/code/register");
        assert_eq!(bodies[0].1["apiKey"], "deadbeef");
        assert_eq!(bodies[0].1["code"], "ABCD1234");
        assert_eq!(bodies[0].1["expiryMinutes"], 5);
    }

    #[tokio::test]
    async fn register_code_propagates_non_success() {
        let mock = MockState::default();
        *mock.fail_seq.lock().unwrap() = vec![true];
        let url = spawn_mock(mock).await;
        let client = Client::new();

        let r = register_code(&client, &url, "id", "ABCD1234", 5).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn empty_temp_key_returned_is_an_error() {
        // Replace the route so it returns an empty tempKey.
        async fn handler(Json(_): Json<Value>) -> Json<Value> {
            Json(serde_json::json!({"tempKey": "", "expiresAt": 0}))
        }
        let app = Router::new()
            .route("/api/session/create", post(|Json(_): Json<Value>| async {
                Json(serde_json::json!({"ok": true}))
            }))
            .route("/api/session/update", post(|Json(_): Json<Value>| async {
                Json(serde_json::json!({"ok": true}))
            }))
            .route("/api/temp-key/create", post(handler));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = Client::new();

        let res = register_session(&client, &url, "id", "https://t").await;
        assert!(res.is_err());
    }
}
