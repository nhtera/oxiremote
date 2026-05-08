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
use crate::secure_file;

/// File name (under the agent data dir) holding the shared secret that the
/// agent attaches as `Authorization: Bearer <secret>` on every mutating
/// discovery-worker POST. 0o600 on Unix; ACL-restricted on Windows. Phase 04
/// / H1 — decouples the routing key (`discovery_id`) from the write
/// credential.
pub const DISCOVERY_SECRET_FILENAME: &str = "discovery_secret";
const DISCOVERY_SECRET_BYTES: usize = 32;

/// Worker-side TTL on the temp key (matches `phase-01-discovery-worker.md`).
pub const TEMP_KEY_EXPIRY_MINUTES: u32 = 30;

/// Worker-side TTL for the permanent-key lookup id. The agent re-registers on
/// every `TunnelUrlChanged`, but tunnels can stay stable for hours; a long TTL
/// keeps cross-origin sk-… pairing working in the no-rotation case.
pub const PERMANENT_LOOKUP_EXPIRY_MINUTES: u32 = 24 * 60;

/// Cadence for the session refresh heartbeat. Without this loop the session
/// record's KV TTL would expire on long-lived stable tunnels and every
/// cross-origin lookup would dangle (tempkey index points at a session that
/// no longer exists). 15 min gives the worker session record (24h TTL) a
/// generous safety margin. Note: `TunnelUrlChanged` also fires on mid-process
/// URL rotations (tunnel.rs stderr parser detects this), but rotations are
/// not guaranteed to happen — the heartbeat is the floor.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 15 * 60;

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

/// Path to the shared discovery-worker secret. Lives in `data_dir` next to
/// other 0o600 files (`signing.key`, `notify.token`).
pub fn discovery_secret_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(DISCOVERY_SECRET_FILENAME)
}

/// Load or first-boot generate the agent's discovery-worker write secret.
///
/// On first call (file absent): generate 32 random bytes, hex-encode, persist
/// 0o600 via `secure_file::write_secret`, log INFO with the value once and
/// emit a `SettingsHint` so the dashboard surfaces a one-time banner. The
/// operator copies it and the worker's `wrangler secret put
/// AGENT_REGISTRATION_SECRET` is set to the same value.
///
/// Subsequent calls: read the existing file. Self-heal mode via
/// `ensure_owner_only` (no-op when permissions are already correct). Never
/// re-logs the value.
pub fn load_or_create_discovery_secret(
    data_dir: &Path,
    event_bus: Option<&Arc<EventBus>>,
) -> Result<String> {
    let path = discovery_secret_path(data_dir);
    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("read discovery secret at {}", path.display()))?;
        let secret = content.trim().to_string();
        if secret.is_empty() {
            return Err(anyhow!(
                "discovery secret at {} is empty — delete it and restart to regenerate",
                path.display()
            ));
        }
        // Self-heal mode if the file is somehow widened by a backup tool.
        if let Err(err) = secure_file::ensure_owner_only(&path) {
            warn!(
                error=%err,
                path=%path.display(),
                "could not enforce owner-only perms on discovery_secret"
            );
        }
        return Ok(secret);
    }

    // First boot: generate 32 random bytes, hex-encode.
    let mut bytes = [0u8; DISCOVERY_SECRET_BYTES];
    rand::rng().fill(&mut bytes);
    let mut secret = String::with_capacity(DISCOVERY_SECRET_BYTES * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(secret, "{b:02x}");
    }
    secure_file::write_secret(&path, secret.as_bytes())
        .with_context(|| format!("persist discovery secret to {}", path.display()))?;

    // Surface the freshly-generated value via three channels so the operator
    // doesn't have to re-fetch via the recovery subcommand:
    //   1. log INFO (visible in TUI / journalctl / docker logs)
    //   2. SettingsHint (dashboard banner — Phase 02 hint plumbing)
    //   3. file at 0o600 (recoverable via `oxiremote config discovery-secret`)
    info!(
        secret = %secret,
        path = %path.display(),
        "discovery_secret generated — copy this value to the worker via \
         `wrangler secret put AGENT_REGISTRATION_SECRET`. \
         Recover later with `oxiremote config discovery-secret`."
    );
    if let Some(bus) = event_bus {
        bus.send(AgentEvent::SettingsHint {
            code: crate::hints::CODE_DISCOVERY_SECRET_GENERATED.to_string(),
            severity: crate::events::HintSeverity::Info,
            message: format!(
                "Discovery worker secret generated. Configure the worker with this value, \
                 then dismiss: {secret}"
            ),
        });
    }
    Ok(secret)
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
/// the manual-entry flow. `permanent_lookup_id` is the SHA-256 prefix the SPA
/// derives from a user-typed `sk-…` to resolve the same way. Both registrations
/// are best-effort — failure leaves QR-scan / OTK paths working.
///
/// `discovery_secret` is the Bearer credential the worker validates (Phase 04
/// / H1). Sent on every POST.
#[allow(clippy::too_many_arguments)]
pub fn spawn_register(
    client: Client,
    discovery_url: String,
    discovery_id: String,
    discovery_secret: String,
    tunnel_url: String,
    temp_key_slot: Arc<StdRwLock<Option<String>>>,
    event_bus: Arc<EventBus>,
    pairing_code: Option<(String, u32)>,
    permanent_lookup_id: Option<String>,
) {
    tokio::spawn(async move {
        match register_with_retry(
            &client,
            &discovery_url,
            &discovery_id,
            &discovery_secret,
            &tunnel_url,
        )
        .await
        {
            Ok(temp_key) => {
                let prefix: String = temp_key.chars().take(4).collect();
                if let Ok(mut g) = temp_key_slot.write() {
                    *g = Some(temp_key);
                }
                info!(prefix = %prefix, "discovery temp key issued");
                event_bus.send(AgentEvent::DiscoveryTempKeyIssued { key_prefix: prefix });

                if let Some((code, mins)) = pairing_code {
                    match register_code(
                        &client,
                        &discovery_url,
                        &discovery_id,
                        &discovery_secret,
                        &code,
                        mins,
                    )
                    .await
                    {
                        Ok(()) => debug!(mins, "pairing code registered with discovery worker"),
                        Err(e) => warn!(error = %e, "code/register failed (manual code-entry will fall back to QR)"),
                    }
                }

                if let Some(lookup_id) = permanent_lookup_id {
                    match register_code(
                        &client,
                        &discovery_url,
                        &discovery_id,
                        &discovery_secret,
                        &lookup_id,
                        PERMANENT_LOOKUP_EXPIRY_MINUTES,
                    )
                    .await
                    {
                        Ok(()) => debug!("permanent-key lookup_id registered with discovery worker"),
                        Err(e) => warn!(error = %e, "permanent-key lookup_id register failed (sk- pairing will fail cross-origin until next rotation)"),
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

/// Re-write the session record with the current tunnel URL so the worker's
/// KV TTL resets. Called by the heartbeat loop — keeps the `session:` row
/// alive when Quick Tunnel never emits another `TunnelUrlChanged`. Single
/// POST to `session/update`; idempotent.
pub fn spawn_refresh_session(
    client: Client,
    discovery_url: String,
    discovery_id: String,
    discovery_secret: String,
    tunnel_url: String,
) {
    tokio::spawn(async move {
        let base = discovery_url.trim_end_matches('/');
        let normalized = normalize_tunnel_url(&tunnel_url);
        let body = SessionUpdateBody {
            api_key: &discovery_id,
            tunnel_url: &normalized,
        };
        match client
            .post(format!("{base}/api/session/update"))
            .bearer_auth(&discovery_secret)
            .json(&body)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                debug!("discovery session heartbeat ok");
            }
            Ok(res) => {
                warn!(status = %res.status(), "discovery session heartbeat returned non-success");
            }
            Err(e) => {
                warn!(error = %e, "discovery session heartbeat POST failed");
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
    discovery_secret: String,
    code: String,
    expiry_minutes: u32,
) {
    tokio::spawn(async move {
        match register_code(
            &client,
            &discovery_url,
            &discovery_id,
            &discovery_secret,
            &code,
            expiry_minutes,
        )
        .await
        {
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
    discovery_secret: &str,
    code: &str,
    expiry_minutes: u32,
) -> Result<()> {
    let base = base.trim_end_matches('/');
    let res = client
        .post(format!("{base}/api/code/register"))
        .bearer_auth(discovery_secret)
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
    discovery_secret: &str,
    tunnel_url: &str,
) -> Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=RETRY_ATTEMPTS {
        match register_session(client, base, discovery_id, discovery_secret, tunnel_url).await {
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
    discovery_secret: &str,
    tunnel_url: &str,
) -> Result<String> {
    let base = base.trim_end_matches('/');
    let normalized = normalize_tunnel_url(tunnel_url);
    let tunnel_url = normalized.as_str();

    // 1) session/create — idempotent upsert.
    let res = client
        .post(format!("{base}/api/session/create"))
        .bearer_auth(discovery_secret)
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
        .bearer_auth(discovery_secret)
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
        .bearer_auth(discovery_secret)
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

    const TEST_SECRET: &str = "test-secret-aaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn register_session_normalizes_bare_hostname() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        register_session(&client, &url, "id", TEST_SECRET, "oxiremote.erai.dev")
            .await
            .unwrap();

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies[1].1["tunnelUrl"], "https://oxiremote.erai.dev");
    }

    #[tokio::test]
    async fn round_trip_posts_three_routes_in_order() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        let temp_key =
            register_session(&client, &url, "deadbeef", TEST_SECRET, "https://t.example")
                .await
                .unwrap();
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

        let temp_key = register_with_retry(&client, &url, "id", TEST_SECRET, "https://t")
            .await
            .unwrap();
        assert_eq!(temp_key, "deadbeefcafe1234deadbeefcafe1234");
    }

    #[tokio::test]
    async fn all_retries_exhausted_returns_error() {
        let mock = MockState::default();
        *mock.fail_seq.lock().unwrap() = vec![true; 16];
        let url = spawn_mock(mock).await;
        let client = Client::new();

        let result = register_with_retry(&client, &url, "id", TEST_SECRET, "https://t").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_code_posts_to_code_register_endpoint() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        register_code(&client, &url, "deadbeef", TEST_SECRET, "ABCD1234", 5)
            .await
            .unwrap();

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].0, "/api/code/register");
        assert_eq!(bodies[0].1["apiKey"], "deadbeef");
        assert_eq!(bodies[0].1["code"], "ABCD1234");
        assert_eq!(bodies[0].1["expiryMinutes"], 5);
    }

    #[tokio::test]
    async fn refresh_session_posts_only_session_update() {
        let mock = MockState::default();
        let url = spawn_mock(mock.clone()).await;
        let client = Client::new();

        spawn_refresh_session(
            client,
            url.clone(),
            "deadbeef".into(),
            TEST_SECRET.into(),
            "https://t.example".into(),
        );
        // Give the spawned task a beat to fire the POST.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let bodies = mock.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1, "heartbeat must hit only session/update");
        assert_eq!(bodies[0].0, "/api/session/update");
        assert_eq!(bodies[0].1["apiKey"], "deadbeef");
        assert_eq!(bodies[0].1["tunnelUrl"], "https://t.example");
    }

    #[tokio::test]
    async fn register_code_propagates_non_success() {
        let mock = MockState::default();
        *mock.fail_seq.lock().unwrap() = vec![true];
        let url = spawn_mock(mock).await;
        let client = Client::new();

        let r = register_code(&client, &url, "id", TEST_SECRET, "ABCD1234", 5).await;
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

        let res = register_session(&client, &url, "id", TEST_SECRET, "https://t").await;
        assert!(res.is_err());
    }

    // Phase 04 / H1 — verify Bearer header is attached on every POST.
    #[tokio::test]
    async fn bearer_header_attached_on_all_register_posts() {
        use std::sync::Mutex;
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();

        async fn capture_auth(
            AxumState(cap): AxumState<Arc<Mutex<Vec<String>>>>,
            headers: axum::http::HeaderMap,
            Json(_): Json<Value>,
        ) -> Json<Value> {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            cap.lock().unwrap().push(auth);
            Json(serde_json::json!({
                "ok": true,
                "tempKey": "deadbeefcafe1234deadbeefcafe1234"
            }))
        }

        let app = Router::new()
            .route("/api/session/create", post(capture_auth))
            .route("/api/session/update", post(capture_auth))
            .route("/api/temp-key/create", post(capture_auth))
            .route("/api/code/register", post(capture_auth))
            .with_state(cap);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = Client::new();

        // Round trip: 3 POSTs from register_session.
        register_session(&client, &url, "id", TEST_SECRET, "https://t.example")
            .await
            .unwrap();
        // 4th POST from register_code.
        register_code(&client, &url, "id", TEST_SECRET, "ABCD1234", 5)
            .await
            .unwrap();

        let auths = captured.lock().unwrap().clone();
        assert_eq!(auths.len(), 4, "expected 4 captured auth headers");
        let expected = format!("Bearer {TEST_SECRET}");
        for (i, a) in auths.iter().enumerate() {
            assert_eq!(a, &expected, "POST {i} missing or wrong Bearer header");
        }
    }

    // Phase 04 / H1 — load_or_create_discovery_secret persists + rereads.
    #[test]
    fn discovery_secret_first_boot_generates_then_rereads() {
        let dir = tempdir_for("disc-sec");
        let first = load_or_create_discovery_secret(&dir, None).unwrap();
        // 32 bytes hex-encoded = 64 chars.
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));

        // Second call returns the same value (no regeneration).
        let second = load_or_create_discovery_secret(&dir, None).unwrap();
        assert_eq!(first, second);

        // File exists at the expected path.
        let p = discovery_secret_path(&dir);
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_secret_persists_with_owner_only_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir_for("disc-mode");
        let _ = load_or_create_discovery_secret(&dir, None).unwrap();
        let p = discovery_secret_path(&dir);
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected 0o600, got {:o}", mode & 0o777);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempdir_for(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "oxi-disc-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
