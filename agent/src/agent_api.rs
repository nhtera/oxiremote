// Localhost-only API under `/api/agent/*`. Surfaces internal agent state and
// event stream to the in-process TUI, system tray, and `/agent` dashboard.
// Route scope enforces tunnel 403 — no auth on these handlers.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::{Event, KeepAlive}},
    routing::{get, post},
};
use futures_util::stream::Stream;
use qrcode::{QrCode, render::svg};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tracing::{info, warn};

use crate::AppState;
use crate::events::AgentEvent;
use crate::{approval, one_time_keys};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/agent/events", get(api_agent_events))
        .route("/api/agent/state", get(api_agent_state))
        .route("/api/agent/logs/recent", get(api_agent_logs_recent))
        .route("/api/agent/qr", get(api_agent_qr))
        .route("/api/agent/keys/one-time", post(api_agent_keys_one_time))
        .route(
            "/api/agent/keys/permanent",
            get(api_agent_keys_permanent_get).post(api_agent_keys_permanent_post),
        )
        .route("/api/agent/approvals/pending", get(api_agent_approvals_pending))
        .route("/api/agent/approvals/{id}/approve", post(api_agent_approve))
        .route("/api/agent/approvals/{id}/reject", post(api_agent_reject))
        .route("/api/agent/settings/auto-approve", post(api_agent_settings_auto_approve))
        .route(
            "/api/agent/proxy/ports",
            get(api_agent_proxy_ports_list).post(api_agent_proxy_ports_set),
        )
        .route("/api/agent/permissions", get(api_agent_permissions))
        .route("/api/agent/permissions/grant", post(api_agent_permissions_grant))
        .route("/api/agent/devices", get(api_agent_devices))
        .route("/api/agent/shutdown", post(api_agent_shutdown))
}

/// POST /api/agent/shutdown — operator-initiated stop from the host dashboard.
/// Localhost-only (route_scope rejects tunnel callers). Returns 202 immediately
/// so the SPA's fetch settles before the process exits ~500 ms later.
async fn api_agent_shutdown(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("shutdown requested via /api/agent/shutdown");
    let _ = state; // currently unused; kept for future graceful-shutdown wiring
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        crate::tui::restore_terminal_if_active();
        std::process::exit(0);
    });
    StatusCode::ACCEPTED
}

async fn api_agent_state(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let tunnel_url = state
        .tunnel_url
        .read()
        .ok()
        .and_then(|g| g.clone());
    let connected_devices = state.terminal_sessions.len();
    let auto_approve = approval::get_auto_approve(&state.db_path);
    // Mirror the latest TunnelStepChanged event so SSE late-joiners can
    // hydrate the 5-step progress card. Shape matches the SSE frame so the
    // client can apply it via the same reducer.
    let tunnel_step = state
        .latest_tunnel_step
        .read()
        .ok()
        .and_then(|g| g.clone())
        .map(|ev| serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null));
    Json(json!({
        "tunnel_url": tunnel_url,
        "tunnel_step": tunnel_step,
        "host_id": state.host_info.host_id,
        "label": state.host_info.label,
        "platform": state.host_info.platform,
        "connected_devices": connected_devices,
        "auto_approve": auto_approve,
    }))
}

/// GET /api/agent/logs/recent — backfill for the `/agent/logs` page when it
/// mounts after the agent has already been running. Returns the last
/// `LOG_RING_CAP` `LogEntry` events in chronological order. Each entry is the
/// same shape as the SSE frame so the client can pass it through unchanged.
async fn api_agent_logs_recent(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let entries: Vec<serde_json::Value> = state
        .recent_logs
        .lock()
        .ok()
        .map(|g| {
            g.iter()
                .filter_map(|ev| serde_json::to_value(ev).ok())
                .collect()
        })
        .unwrap_or_default();
    Json(json!({ "entries": entries }))
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    filter: Option<String>,
}

async fn api_agent_events(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_bus.subscribe();
    // `?filter=log` → only stream `LogEntry` events. Used by /agent/logs.
    let log_only = matches!(q.filter.as_deref(), Some("log"));
    let stream = BroadcastStream::new(rx).filter_map(move |msg| match msg {
        Ok(event) => {
            if log_only && !matches!(event, AgentEvent::LogEntry { .. }) {
                None
            } else {
                Some(Ok(event_to_sse(&event)))
            }
        }
        Err(_) => None, // drop lagged frames; client may GET /api/agent/state to resync
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

fn event_to_sse(event: &AgentEvent) -> Event {
    let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    Event::default().data(payload)
}

#[derive(serde::Deserialize)]
struct QrQuery {
    url: String,
}

async fn api_agent_qr(
    axum::extract::Query(q): axum::extract::Query<QrQuery>,
) -> axum::response::Response {
    use axum::http::header;
    match QrCode::new(q.url.as_bytes()) {
        Ok(code) => {
            let svg_string = code
                .render::<svg::Color<'_>>()
                .min_dimensions(256, 256)
                .dark_color(svg::Color("#111217"))
                .light_color(svg::Color("#ffffff"))
                .build();
            ([(header::CONTENT_TYPE, "image/svg+xml")], svg_string).into_response()
        }
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// POST /api/agent/keys/one-time — generate a new OTK (invalidates prior live token).
async fn api_agent_keys_one_time(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match one_time_keys::generate_otk(&state.db_path, None) {
        Ok(rec) => {
            let prefix: String = rec.token.chars().take(4).collect();
            info!(token_prefix = %prefix, "OTK issued");
            state.event_bus.send(AgentEvent::OtkIssued { token_prefix: prefix });

            let tunnel_url = state.tunnel_url.read().ok().and_then(|g| g.clone());
            let qr_url = match tunnel_url {
                Some(ref host) => format!("https://{host}/login?k={}", rec.token),
                None => format!("http://localhost:8787/login?k={}", rec.token),
            };

            (
                StatusCode::OK,
                Json(json!({
                    "token": rec.token,
                    "expires_at": rec.expires_at,
                    "qr_url": qr_url,
                })),
            )
                .into_response()
        }
        Err(err) => {
            warn!(error=%err, "OTK generation failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/agent/approvals/pending — list devices awaiting approval.
async fn api_agent_approvals_pending(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match approval::list_pending(&state.db_path) {
        Ok(devices) => (StatusCode::OK, Json(devices)).into_response(),
        Err(err) => {
            warn!(error=%err, "list pending approvals failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /api/agent/approvals/{id}/approve
async fn api_agent_approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match approval::approve_device(&state.db_path, &id) {
        Ok(()) => {
            info!(device_id = %id, "device approved");
            state
                .event_bus
                .send(AgentEvent::DeviceApproved { device_id: id });
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, device_id=%id, "approve device failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /api/agent/approvals/{id}/reject
async fn api_agent_reject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match approval::reject_device(&state.db_path, &id) {
        Ok(()) => {
            info!(device_id = %id, "device rejected");
            state
                .event_bus
                .send(AgentEvent::DeviceRejected { device_id: id });
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, device_id=%id, "reject device failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/agent/proxy/ports — surface discovered listeners alongside the
/// persisted allowlist so the UI can render a single toggle list.
async fn api_agent_proxy_ports_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let discovered = state.local_sites.read().await.clone();
    let allowed: Vec<u16> = {
        let guard = state.proxy_allowed_ports.read().unwrap();
        let mut out: Vec<u16> = guard.iter().copied().collect();
        out.sort_unstable();
        out
    };
    Json(json!({
        "discovered": discovered,
        "allowed": allowed,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ProxyPortBody {
    port: u16,
    enabled: bool,
}

/// POST /api/agent/proxy/ports — flip a single port on/off. Persists to the
/// settings table and updates the in-memory DashSet so the next request
/// observes the change without restart.
async fn api_agent_proxy_ports_set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ProxyPortBody>,
) -> impl IntoResponse {
    if body.port == 0 {
        return (StatusCode::BAD_REQUEST, "port required").into_response();
    }
    match crate::proxy::set_allowed(&state, body.port, body.enabled) {
        Ok(allowed) => {
            info!(port = body.port, enabled = body.enabled, "proxy port toggled");
            (StatusCode::OK, Json(json!({ "allowed": allowed }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, "proxy ports persist failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/agent/permissions — TCC / Accessibility status for the desktop
/// pipeline. macOS is the only OS where this matters in practice; Linux and
/// Windows always report `true`. Returns 503 when the agent was built without
/// the `desktop` feature so the UI can render a "not supported" badge.
async fn api_agent_permissions() -> impl IntoResponse {
    #[cfg(feature = "desktop")]
    {
        let status = tokio::task::spawn_blocking(desktop::permissions::PermissionStatus::check)
            .await
            .unwrap_or(desktop::permissions::PermissionStatus {
                screen_recording: false,
                accessibility: false,
            });
        (
            StatusCode::OK,
            Json(json!({
                "screen_recording": status.screen_recording,
                "accessibility": status.accessibility,
                "platform": std::env::consts::OS,
                "supported": true,
            })),
        )
            .into_response()
    }
    #[cfg(not(feature = "desktop"))]
    {
        (
            StatusCode::OK,
            Json(json!({
                "screen_recording": false,
                "accessibility": false,
                "platform": std::env::consts::OS,
                "supported": false,
            })),
        )
            .into_response()
    }
}

#[derive(Deserialize)]
struct PermissionsGrantBody {
    /// One of `screen_recording` or `accessibility`. Anything else is rejected.
    kind: String,
}

/// POST /api/agent/permissions/grant — open the relevant System Settings
/// pane. macOS only; other platforms return 501. The `open` crate handles
/// the URL scheme; we never inject user input into the URL.
async fn api_agent_permissions_grant(
    Json(body): Json<PermissionsGrantBody>,
) -> impl IntoResponse {
    #[cfg(target_os = "macos")]
    {
        let url = match body.kind.as_str() {
            "screen_recording" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            "accessibility" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            _ => {
                return (StatusCode::BAD_REQUEST, "unknown permission kind").into_response();
            }
        };
        match open::that(url) {
            Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            Err(err) => {
                warn!(error=%err, "failed to open settings pane");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = body; // suppress unused warning
        (
            StatusCode::NOT_IMPLEMENTED,
            "permission granting only implemented on macOS",
        )
            .into_response()
    }
}

/// GET /api/agent/keys/permanent — return metadata (last4 + created_at) for
/// the host's permanent dashboard API key. Never returns the hash or plaintext.
/// Returns 404 when no permanent key has been generated yet.
async fn api_agent_keys_permanent_get(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match crate::auth::get_permanent_key_meta(&state.db_path) {
        Ok(Some((last4, created_at))) => (
            StatusCode::OK,
            Json(json!({ "last4": last4, "created_at": created_at })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no permanent key generated yet" })),
        )
            .into_response(),
        Err(err) => {
            warn!(error=%err, "get permanent key meta failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /api/agent/keys/permanent — rotate the host's permanent dashboard API
/// key. Generates a fresh `sk-<base64>` token, stores the Argon2id hash, and
/// returns the plaintext **once**. Emits `AgentEvent::PermanentKeyRotated` so
/// other dashboard tabs can refresh their metadata.
///
/// This key is stored in the `settings` table (not `trusted_devices`) because
/// it belongs to the host/dashboard itself, not to any paired client device.
/// Per-device keys in `trusted_devices.api_key_hash` are unaffected.
async fn api_agent_keys_permanent_post(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db_path = state.db_path.clone();
    // Argon2id hashing is CPU-intensive; run on the blocking thread pool.
    let result = tokio::task::spawn_blocking(move || {
        crate::auth::rotate_permanent_key(&db_path)
    })
    .await;

    match result {
        Ok(Ok((api_key, last4, created_at))) => {
            info!(last4 = %last4, "permanent key rotated");
            state
                .event_bus
                .send(AgentEvent::PermanentKeyRotated { last4: last4.clone() });
            (
                StatusCode::OK,
                Json(json!({
                    "api_key": api_key,
                    "last4": last4,
                    "created_at": created_at,
                })),
            )
                .into_response()
        }
        Ok(Err(err)) => {
            warn!(error=%err, "permanent key rotation failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(err) => {
            warn!(error=%err, "spawn_blocking panic during permanent key rotation");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/agent/devices — host-only device list. The user-facing
/// `/api/devices` endpoint requires a session cookie, but the agent dashboard
/// at `/agent` runs before any device has paired, so we expose an
/// auth-free localhost-only mirror. `route_scope::LOCALHOST_PREFIXES`
/// already restricts `/api/agent/*` to loopback callers.
async fn api_agent_devices(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::auth::list_trusted_devices(&state.db_path) {
        Ok(devices) => (StatusCode::OK, Json(devices)).into_response(),
        Err(err) => {
            warn!(error=%err, "list trusted devices failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct AutoApproveBody {
    enabled: bool,
}

/// POST /api/agent/settings/auto-approve — upsert the auto_approve setting.
async fn api_agent_settings_auto_approve(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AutoApproveBody>,
) -> impl IntoResponse {
    match approval::set_auto_approve(&state.db_path, body.enabled) {
        Ok(()) => {
            info!(enabled = body.enabled, "auto_approve setting updated");
            (StatusCode::OK, Json(json!({ "ok": true, "enabled": body.enabled }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, "set auto_approve failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dashmap::DashMap;

    use crate::{auth, db::{init_db, now_ts}, local_sites, AppState};

    fn test_db(name: &str) -> PathBuf {
        let db_path = std::env::temp_dir().join(format!(
            "oxiremote-agentapi-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&db_path);
        init_db(&db_path).unwrap();
        db_path
    }

    fn test_state(name: &str) -> std::sync::Arc<AppState> {
        let db_path = test_db(name);
        let data_dir = db_path.parent().unwrap().join(format!("oxi-data-agentapi-{name}"));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::sync::Arc::new(AppState {
            db_path,
            signing_key: b"01234567890123456789012345678901".to_vec(),
            secure_cookies: false,
            terminal_sessions: DashMap::new(),
            preview_targets: DashMap::new(),
            preview_health: DashMap::new(),
            local_sites: local_sites::new_cache(),
            proxy_allowed_ports: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            pairing_attempts: DashMap::new(),
            workspace_root: PathBuf::from("."),
            host_info: crate::host::HostInfo {
                host_id: "test-host-id".to_string(),
                label: "test-host".to_string(),
                platform: "test".to_string(),
            },
            vapid_keys: std::sync::Arc::new(
                crate::push::load_or_create_vapid(&data_dir).unwrap(),
            ),
            notify_token: "test-token".to_string(),
            http_client: reqwest::Client::new(),
            preview_client: reqwest::Client::new(),
            rate_limiter: std::sync::Arc::new(crate::security::rate_limit::RateLimiter::new()),
            event_bus: crate::events::EventBus::new(),
            tunnel_url: std::sync::Arc::new(std::sync::RwLock::new(None)),
            latest_tunnel_step: std::sync::Arc::new(std::sync::RwLock::new(None)),
            recent_logs: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            desktop_available: false,
            desktop_service: None,
        })
    }

    /// GET returns metadata only — last4 + created_at, no hash, no plaintext.
    #[test]
    fn permanent_key_get_returns_last4_only() {
        let state = test_state("pk-get");
        // No key yet — meta returns None.
        assert!(auth::get_permanent_key_meta(&state.db_path).unwrap().is_none());

        // Rotate once.
        let (plaintext, last4, created_at) = auth::rotate_permanent_key(&state.db_path).unwrap();
        let (meta_last4, meta_created_at) = auth::get_permanent_key_meta(&state.db_path)
            .unwrap()
            .expect("meta should exist after rotation");

        // Metadata matches.
        assert_eq!(meta_last4, last4);
        assert_eq!(meta_created_at, created_at);

        // Sanity: meta does NOT contain the full key.
        assert!(meta_last4.len() == 4);
        assert!(!plaintext.is_empty());
        assert!(plaintext.starts_with("sk-"));
        // last4 is the tail of the full plaintext key.
        assert!(plaintext.ends_with(&last4));
    }

    /// POST rotates the Argon2 hash — the new hash matches the new key.
    #[test]
    fn permanent_key_post_rotates_hash() {
        let state = test_state("pk-rotate");
        let (key1, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();
        let (key2, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key must no longer verify.
        assert!(!auth::verify_permanent_key(&state.db_path, &key1));
        // New key must verify.
        assert!(auth::verify_permanent_key(&state.db_path, &key2));
    }

    /// After rotation the old plaintext fails verification — it's invalidated.
    #[test]
    fn permanent_key_post_invalidates_old_key() {
        let state = test_state("pk-invalidate");
        let (old_key, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key verifies before rotation.
        assert!(auth::verify_permanent_key(&state.db_path, &old_key));

        // Rotate to new key.
        let (_new_key, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key no longer authenticates.
        assert!(!auth::verify_permanent_key(&state.db_path, &old_key));
    }
}
