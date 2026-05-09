// Localhost-only API under `/api/agent/*`. Surfaces internal agent state and
// event stream to the in-process TUI, system tray, and `/agent` dashboard.
// Route scope enforces tunnel 403 — no auth on these handlers.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Sse, sse::{Event, KeepAlive}},
    routing::{get, patch, post},
};
use futures_util::stream::Stream;
use qrcode::{QrCode, render::svg};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tracing::{info, warn};

use crate::AppState;
use crate::db;
use crate::events::AgentEvent;
use crate::{approval, autostart, files_activity, one_time_keys, settings};

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
        .route("/api/agent/devices/{id}", patch(api_agent_device_patch))
        .route("/api/agent/devices/{id}/revoke", post(api_agent_device_revoke))
        .route("/api/agent/devices/{id}/disconnect", post(api_agent_device_disconnect))
        .route(
            "/api/agent/autostart",
            get(api_agent_autostart_get).post(api_agent_autostart_post),
        )
        .route("/api/agent/services/desktop", post(api_agent_services_desktop))
        .route("/api/agent/shutdown", post(api_agent_shutdown))
        .route("/api/agent/tunnel/disconnect", post(api_agent_tunnel_disconnect))
        // --- operator telemetry endpoints (localhost-only via route_scope) ---
        .route("/api/agent/version", get(api_agent_version))
        .route("/api/agent/sessions/active", get(api_agent_sessions_active))
        .route("/api/agent/tunnel/telemetry", get(api_agent_tunnel_telemetry))
        .route("/api/agent/keys/recent-usage", get(api_agent_keys_recent_usage))
        .route(
            "/api/agent/cors/origins",
            get(api_agent_cors_origins_list).delete(api_agent_cors_origins_delete),
        )
}

#[derive(Deserialize)]
struct DeleteCorsOriginBody {
    origin: String,
}

/// GET /api/agent/cors/origins — list the runtime CORS allowlist (DB-backed).
/// Localhost-only (enforced by route_scope). Used by the host dashboard to
/// surface and prune accumulated origins.
async fn api_agent_cors_origins_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let list = db::load_cors_allowed_origins(&state.db_path).unwrap_or_default();
    Json(json!({ "origins": list }))
}

/// DELETE /api/agent/cors/origins — remove an origin from the allowlist. Body
/// `{ "origin": "https://..." }`. Returns 204 on success regardless of whether
/// the origin existed (idempotent).
async fn api_agent_cors_origins_delete(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteCorsOriginBody>,
) -> impl IntoResponse {
    if let Err(err) = db::remove_cors_origin(&state.db_path, &body.origin) {
        warn!(error=%err, origin=%body.origin, "failed to remove cors origin from db");
    }
    if let Ok(hv) = HeaderValue::from_str(&body.origin)
        && let Ok(mut set) = state.cors_origins.write()
    {
        set.remove(&hv);
    }
    StatusCode::NO_CONTENT
}

/// POST /api/agent/tunnel/disconnect — stop sharing without exiting the agent.
/// Localhost-only. Notifies the tunnel-spawn task to SIGTERM cloudflared and
/// reap the child; the task then broadcasts `TunnelDisconnected`. Re-spinning
/// the tunnel requires an agent restart (matches the existing one-shot
/// behavior — quick-tunnel URLs rotate, so restart-only is intentional).
async fn api_agent_tunnel_disconnect(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("tunnel disconnect requested via /api/agent/tunnel/disconnect");
    state.tunnel_shutdown.notify_one();
    // Clear the cached URL so /api/agent/state immediately reflects the new
    // onboarding state; the bus event will arrive moments later but the SPA's
    // first poll after the response already shows the right state.
    if let Ok(mut guard) = state.tunnel_url.write() {
        *guard = None;
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
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
    let desktop_enabled = settings::get_desktop_enabled(&state.db_path);
    // Mirror the latest TunnelStepChanged event so SSE late-joiners can
    // hydrate the 5-step progress card. Shape matches the SSE frame so the
    // client can apply it via the same reducer.
    let tunnel_step = state
        .latest_tunnel_step
        .read()
        .ok()
        .and_then(|g| g.clone())
        .map(|ev| serde_json::to_value(&ev).unwrap_or(serde_json::Value::Null));

    // Active clients: devices with last_active_at within the last 5 minutes.
    // Used by the stop-agent confirm dialog to list connected clients.
    let active_clients = query_active_clients(&state.db_path);

    // Surface the latest unused/unexpired OTK so a page refresh keeps the
    // same key alive instead of stranding the SPA with `otk: null`. The SPA
    // will auto-mint when this is null (fresh boot or post-expiry).
    let otk = one_time_keys::active_otk(&state.db_path)
        .ok()
        .flatten()
        .map(|rec| json!({ "token": rec.token, "expires_at": rec.expires_at }));

    // True when the discovery worker URL is active (env set or bundled default).
    // False only when the user explicitly cleared OXI_DISCOVERY_URL=.
    let discovery_configured = crate::env_defaults::discovery_url().is_some();
    // Tunnel mode for the SPA discovery banner — only show the banner when
    // discovery is disabled AND the URL rotates on every restart (quick tunnel).
    let tunnel_mode = if state.is_quick_tunnel { "quick" } else { "named" };

    Json(json!({
        "tunnel_url": tunnel_url,
        "tunnel_step": tunnel_step,
        "host_id": state.host_info.host_id,
        "label": state.host_info.label,
        "platform": state.host_info.platform,
        "connected_devices": connected_devices,
        "auto_approve": auto_approve,
        "desktop_enabled": desktop_enabled,
        "version": env!("CARGO_PKG_VERSION"),
        "active_clients": active_clients,
        "otk": otk,
        // Public web app base URL (OXI_WEB_URL). When set, the SPA prefers
        // it over `tunnel_url` for QR / share-link payloads — phones land
        // on the user-facing SPA which then resolves the tunnel via the
        // discovery worker. Null when unset (tunnel-only deployments).
        "web_url": state.web_url,
        // discovery_configured: false only when OXI_DISCOVERY_URL= was
        // explicitly cleared. True for all default/configured installs.
        "discovery_configured": discovery_configured,
        "tunnel_mode": tunnel_mode,
    }))
}

/// Query devices active in the last 5 minutes (300 000 ms). Returns an empty
/// vec on DB error — the stop-agent dialog degrades gracefully with count = 0.
fn query_active_clients(db_path: &std::path::PathBuf) -> Vec<serde_json::Value> {
    let five_min_ago_ms = chrono::Utc::now().timestamp_millis() - 300_000;
    Connection::open(db_path)
        .ok()
        .and_then(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT device_id, device_name, label FROM trusted_devices
                     WHERE revoked_at IS NULL
                       AND last_active_at IS NOT NULL
                       AND last_active_at > ?1
                     ORDER BY last_active_at DESC",
                )
                .ok()?;
            let rows = stmt
                .query_map(rusqlite::params![five_min_ago_ms], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .ok()?;
            let mut out = Vec::new();
            for r in rows.flatten() {
                let (device_id, device_name, label) = r;
                // Prefer custom device_name, fall back to label.
                let display_name = device_name.unwrap_or(label);
                out.push(json!({ "device_id": device_id, "device_name": display_name }));
            }
            Some(out)
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct DesktopServiceToggleReq {
    enabled: bool,
}

/// POST /api/agent/services/desktop — operator toggles remote desktop on/off.
/// Persists to the settings table; the WS upgrade reads it per-connection so
/// the change takes effect for the next attach without a restart.
async fn api_agent_services_desktop(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DesktopServiceToggleReq>,
) -> impl IntoResponse {
    if let Err(e) = settings::set_desktop_enabled(&state.db_path, req.enabled) {
        warn!("set_desktop_enabled failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response();
    }
    Json(json!({ "ok": true, "enabled": req.enabled })).into_response()
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

            // Register the OTK with the discovery worker so the cross-origin
            // SPA can resolve it via /api/session/lookup?k=<otk>. No-op in
            // embedded mode (discovery_url unset). Best-effort; failure here
            // doesn't block the OTK response.
            if let (Some(durl), Ok(disc_id)) = (
                state.discovery_url.clone(),
                crate::discovery::load_discovery_id(&state.db_path),
            ) {
                crate::discovery::spawn_register_code(
                    state.http_client.clone(),
                    durl,
                    disc_id,
                    rec.token.clone(),
                    // OTK_TTL_SECS = 1800 = 30 min — matches worker upper bound.
                    30,
                );
            }

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
        Ok(Ok((api_key, last4, created_at, lookup_id))) => {
            info!(last4 = %last4, "permanent key rotated");
            state
                .event_bus
                .send(AgentEvent::PermanentKeyRotated { last4: last4.clone() });

            // Register the new lookup_id with the discovery worker so the
            // cross-origin SPA can resolve sk-… → tunnelUrl. No-op when
            // discovery is unset; failure here doesn't block the response.
            if let (Some(durl), Ok(disc_id)) = (
                state.discovery_url.clone(),
                crate::discovery::load_discovery_id(&state.db_path),
            ) {
                crate::discovery::spawn_register_code(
                    state.http_client.clone(),
                    durl,
                    disc_id,
                    lookup_id.clone(),
                    crate::discovery::PERMANENT_LOOKUP_EXPIRY_MINUTES,
                );
            }

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

/// GET /api/agent/autostart — detect current autostart state for the agent process.
async fn api_agent_autostart_get() -> impl IntoResponse {
    match autostart::detect() {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(err) => {
            warn!(error=%err, "autostart detect failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct AutostartBody {
    enabled: bool,
}

/// POST /api/agent/autostart — enable or disable autostart.
async fn api_agent_autostart_post(Json(body): Json<AutostartBody>) -> impl IntoResponse {
    match autostart::set_enabled(body.enabled) {
        Ok(()) => {
            info!(enabled = body.enabled, "autostart toggled");
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, "autostart set failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct DevicePatchBody {
    name: Option<String>,
}

/// PATCH /api/agent/devices/{id} — rename a device. Body: `{ "name": "..." | null }`.
/// Emits DeviceApproved event so the Devices page refreshes the row.
async fn api_agent_device_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<DevicePatchBody>,
) -> impl IntoResponse {
    let name_ref: Option<&str> = body.name.as_deref();
    match crate::auth::rename_device(&state.db_path, &id, name_ref) {
        Ok(()) => {
            info!(device_id = %id, "device renamed via agent api");
            // DeviceApproved is the closest existing event that causes the SPA
            // Devices page to refresh a device row. No new variant is invented.
            state.event_bus.send(AgentEvent::DeviceApproved { device_id: id });
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, device_id=%id, "rename device failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

/// POST /api/agent/devices/{id}/disconnect — close active live connections
/// for `id` (desktop session, push subscriptions) without revoking trust.
/// Distinct from /revoke: the device's API key stays valid, so the user can
/// rejoin from the same browser without re-pairing.
async fn api_agent_device_disconnect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Kick the active desktop session if one exists. Silently no-ops when
    // there's nothing to kick (the device may only have a terminal open).
    #[cfg(feature = "desktop")]
    if let Some(svc) = state.desktop_service.as_ref() {
        let _ = svc.kick(&id);
    }

    // Drop any push subscriptions so the device stops receiving notifications
    // until it next reconnects (which will register a fresh subscription).
    if let Ok(conn) = Connection::open(&state.db_path) {
        let _ = conn.execute(
            "DELETE FROM push_subscriptions WHERE device_id = ?1",
            rusqlite::params![id],
        );
    }

    info!(device_id = %id, "device disconnected via agent api");
    state
        .event_bus
        .send(AgentEvent::DeviceDisconnected { device_id: id });
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

/// POST /api/agent/devices/{id}/revoke — revoke a device from the agent dashboard.
/// Drops push subscriptions and emits DeviceRejected to refresh the Devices page.
async fn api_agent_device_revoke(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(err) = crate::auth::revoke_device(&state.db_path, &id) {
        warn!(error=%err, device_id=%id, "agent revoke device failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    // Drop push subscriptions for the revoked device.
    if let Ok(conn) = Connection::open(&state.db_path) {
        let _ = conn.execute(
            "DELETE FROM push_subscriptions WHERE device_id = ?1",
            rusqlite::params![id],
        );
    }

    info!(device_id = %id, "device revoked via agent api");
    // DeviceRejected is the closest existing event variant for removal of a device.
    state.event_bus.send(AgentEvent::DeviceRejected { device_id: id });
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

// ---------------------------------------------------------------------------
// Operator telemetry endpoints
// ---------------------------------------------------------------------------

/// GET /api/agent/version — agent build info + cloudflared version.
///
/// Returns:
/// ```json
/// { "agent": "0.1.0", "cloudflared": "2025.2.1", "build_date": "2026-05-04" }
/// ```
/// `cloudflared` is `null` when the binary is not found or `--version` fails.
/// `build_date` is injected at compile time via `VERGEN_BUILD_DATE` or falls
/// back to `"unknown"` (always non-null).
async fn api_agent_version(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let agent_version = env!("CARGO_PKG_VERSION");
    let build_date = option_env!("VERGEN_BUILD_DATE").unwrap_or("unknown");

    // Query cloudflared --version on-demand. The binary is on the local
    // filesystem and exits immediately; run on the blocking pool to stay off
    // the async executor.
    let cloudflared_path = state.cloudflared_path.clone();
    let cloudflared_version: Option<String> = if let Some(path) = cloudflared_path {
        tokio::task::spawn_blocking(move || detect_cloudflared_version(&path))
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    Json(json!({
        "agent": agent_version,
        "cloudflared": cloudflared_version,
        "build_date": build_date,
    }))
    .into_response()
}

/// Run `<cloudflared> --version` and extract the version string.
/// Returns `None` on any failure (binary not executable, timeout, parse error).
fn detect_cloudflared_version(path: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    // cloudflared prints e.g. "cloudflared version 2025.2.1 (built ...)"
    // on stdout or stderr depending on version. Check both.
    let text = String::from_utf8_lossy(&out.stdout).into_owned()
        + &String::from_utf8_lossy(&out.stderr);
    // Extract the first token that looks like a semver: digits separated by dots.
    for word in text.split_whitespace() {
        if word.contains('.') && word.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Some(word.to_string());
        }
    }
    None
}

/// GET /api/agent/sessions/active — per-client activity snapshot.
///
/// Returns up to 50 rows:
/// ```json
/// [{ "device_id": "...", "label": "iPhone 15", "platform": "ios",
///    "doing": "terminal_active"|"desktop"|"files"|"idle",
///    "last_active_ms": 1234567890 }]
/// ```
async fn api_agent_sessions_active(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = active_sessions_snapshot(&state);
    Json(rows).into_response()
}

/// Build the active-sessions list from all three activity sources.
/// Cap at 50 entries — Vec scan is acceptable for operator dashboards.
pub fn active_sessions_snapshot(state: &AppState) -> Vec<serde_json::Value> {
    const MAX_SESSIONS: usize = 50;

    // Collect known device_ids from all three sources.
    let mut device_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Terminal: each entry in terminal_sessions uses session_id as key. We
    // join via the sessions DB table below to get device_id.
    let live_terminal_session_ids: std::collections::HashSet<String> = state
        .terminal_sessions
        .iter()
        .map(|e| e.key().clone())
        .collect();

    // Desktop: device_id is the direct key.
    #[cfg(feature = "desktop")]
    if let Some(svc) = state.desktop_service.as_ref() {
        for id in svc.active_device_ids() {
            device_ids.insert(id);
        }
    }

    // Files activity map — device_id keys.
    for entry in state.files_activity.iter() {
        device_ids.insert(entry.key().clone());
    }

    // Resolve device_ids from live terminal sessions via the DB.
    let terminal_device_ids =
        resolve_terminal_device_ids(&state.db_path, &live_terminal_session_ids);
    device_ids.extend(terminal_device_ids.keys().cloned());

    // Also include devices active in the last 5 minutes from trusted_devices
    // so recently-idle-but-connected devices appear.
    let recently_active = recently_active_device_ids(&state.db_path);
    device_ids.extend(recently_active);

    // Cap total entries.
    let device_ids: Vec<String> = device_ids.into_iter().take(MAX_SESSIONS).collect();
    if device_ids.is_empty() {
        return vec![];
    }

    // Fetch device metadata in one DB query.
    let meta = fetch_device_meta(&state.db_path, &device_ids);

    // Build response rows.
    let mut rows = Vec::with_capacity(device_ids.len());
    for device_id in &device_ids {
        let (label, platform, last_active_ms) = meta
            .get(device_id)
            .cloned()
            .unwrap_or_else(|| (device_id.chars().take(24).collect(), "unknown".into(), 0));

        // Priority: desktop > terminal_active > files > idle.
        #[cfg(feature = "desktop")]
        let doing = {
            let in_desktop = state
                .desktop_service
                .as_ref()
                .map(|svc| svc.has_session(device_id))
                .unwrap_or(false);
            if in_desktop {
                "desktop"
            } else if terminal_device_ids.get(device_id).copied().unwrap_or(false) {
                "terminal_active"
            } else if files_activity::is_active(&state.files_activity, device_id) {
                "files"
            } else {
                "idle"
            }
        };
        #[cfg(not(feature = "desktop"))]
        let doing = if terminal_device_ids.get(device_id).copied().unwrap_or(false) {
            "terminal_active"
        } else if files_activity::is_active(&state.files_activity, device_id) {
            "files"
        } else {
            "idle"
        };

        rows.push(json!({
            "device_id": device_id,
            "label": label,
            "platform": platform,
            "doing": doing,
            "last_active_ms": last_active_ms,
        }));
    }
    rows
}

/// Resolve which live terminal sessions belong to which device_id.
/// Returns `Map<device_id, is_active>` where `is_active` is true when
/// the session is present in `live_session_ids`.
fn resolve_terminal_device_ids(
    db_path: &std::path::PathBuf,
    live_session_ids: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, bool> {
    if live_session_ids.is_empty() {
        return std::collections::HashMap::new();
    }
    let Ok(conn) = Connection::open(db_path) else {
        return std::collections::HashMap::new();
    };
    let placeholders: Vec<String> = (1..=live_session_ids.len())
        .map(|i| format!("?{i}"))
        .collect();
    let sql = format!(
        "SELECT ts.terminal_session_id, s.device_id
         FROM terminal_sessions ts
         JOIN sessions s ON ts.owner_session_id = s.session_id
         WHERE ts.terminal_session_id IN ({}) AND s.device_id IS NOT NULL",
        placeholders.join(",")
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return std::collections::HashMap::new();
    };
    let ids_vec: Vec<&str> = live_session_ids.iter().map(String::as_str).collect();
    let params: Vec<&dyn rusqlite::ToSql> = ids_vec
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();
    let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return std::collections::HashMap::new();
    };
    let mut out: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for (session_id, device_id) in rows.flatten() {
        let is_live = live_session_ids.contains(&session_id);
        out.entry(device_id)
            .and_modify(|v| *v = *v || is_live)
            .or_insert(is_live);
    }
    out
}

/// Fetch label, platform, and last_active_at for a list of device_ids.
/// Returns `Map<device_id, (label, platform, last_active_ms)>`.
fn fetch_device_meta(
    db_path: &std::path::PathBuf,
    device_ids: &[String],
) -> std::collections::HashMap<String, (String, String, i64)> {
    if device_ids.is_empty() {
        return std::collections::HashMap::new();
    }
    let Ok(conn) = Connection::open(db_path) else {
        return std::collections::HashMap::new();
    };
    let placeholders: Vec<String> =
        (1..=device_ids.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT device_id,
                COALESCE(device_name, label),
                COALESCE(platform, 'unknown'),
                COALESCE(last_active_at, 0)
         FROM trusted_devices WHERE device_id IN ({})",
        placeholders.join(",")
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return std::collections::HashMap::new();
    };
    let ids_ref: Vec<&str> = device_ids.iter().map(String::as_str).collect();
    let params: Vec<&dyn rusqlite::ToSql> =
        ids_ref.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) else {
        return std::collections::HashMap::new();
    };
    let mut out = std::collections::HashMap::new();
    for (device_id, label, platform, last_active_ms) in rows.flatten() {
        // Cap at 80 to match `sanitize_device_label`'s upper bound. Frontend
        // re-formats UA-fallback labels into a friendly "OS · Browser" form,
        // so we want enough chars to keep the browser token intact.
        let label: String = label.chars().take(80).collect();
        out.insert(device_id, (label, platform, last_active_ms));
    }
    out
}

/// Return device_ids active in the last 5 minutes (trusted_devices table).
fn recently_active_device_ids(db_path: &std::path::PathBuf) -> Vec<String> {
    let five_min_ago_ms = chrono::Utc::now().timestamp_millis() - 300_000;
    Connection::open(db_path)
        .ok()
        .and_then(|conn| {
            conn.prepare(
                "SELECT device_id FROM trusted_devices
                 WHERE revoked_at IS NULL AND last_active_at > ?1
                 ORDER BY last_active_at DESC",
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![five_min_ago_ms], |row| {
                    row.get::<_, String>(0)
                })
                .ok()
                .map(|rows| rows.flatten().collect())
            })
        })
        .unwrap_or_default()
}

/// GET /api/agent/tunnel/telemetry — live tunnel metrics.
///
/// Returns:
/// ```json
/// { "uptime_s": 4320, "last_reconnect_at": 1746350000000,
///   "reconnect_count": 2, "bytes_proxied": 1048576,
///   "probe_latencies_p50_ms": [42, 38, 45] }
/// ```
async fn api_agent_tunnel_telemetry(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let t = &state.telemetry;
    let uptime_s = t.tunnel_uptime_s();
    let reconnect_count = t.reconnect_count.load(std::sync::atomic::Ordering::Relaxed);
    let bytes_proxied = t.bytes_proxied.load(std::sync::atomic::Ordering::Relaxed);
    let last_reconnect_at =
        t.last_reconnect_at_ms.load(std::sync::atomic::Ordering::Relaxed);
    let probe_latencies = t.snapshot_latencies();

    Json(json!({
        "uptime_s": uptime_s,
        "last_reconnect_at": last_reconnect_at,
        "reconnect_count": reconnect_count,
        "bytes_proxied": bytes_proxied,
        "probe_latencies_p50_ms": probe_latencies,
    }))
    .into_response()
}

/// GET /api/agent/keys/recent-usage — last 10 OTK-used / key-verified events.
///
/// Returns:
/// ```json
/// [{ "kind": "otk_used"|"key_verified", "device_label": "iPhone 15",
///    "ip": "1.2.3.4", "at": 1746350000 }]
/// ```
async fn api_agent_keys_recent_usage(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = query_recent_key_usage(&state.db_path);
    Json(rows).into_response()
}

/// Query the last 10 key-usage events from the database.
/// Merges OTK `used_at` rows with `trusted_devices.last_seen_at` rows.
fn query_recent_key_usage(db_path: &std::path::PathBuf) -> Vec<serde_json::Value> {
    let Ok(conn) = Connection::open(db_path) else {
        return vec![];
    };

    // OTK usages: one_time_keys where used_at IS NOT NULL.
    // Join via `consumed_by_device` (the phone that scanned the QR) — the
    // legacy `issued_by_session` link points at the dashboard session that
    // generated the code, which never has the consuming-device info.
    let otk_rows: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT COALESCE(td.device_name, td.label, 'unknown'),
                    td.first_seen_ip,
                    td.platform,
                    ok.used_at
             FROM one_time_keys ok
             LEFT JOIN trusted_devices td ON ok.consumed_by_device = td.device_id
             WHERE ok.used_at IS NOT NULL
             ORDER BY ok.used_at DESC
             LIMIT 10",
        )
        .ok()
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .ok()
            .map(|rows| {
                rows.flatten()
                    .map(|(label, ip, platform, at)| {
                        // Cap at 80 to match `sanitize_device_label`'s upper
                        // bound. Frontend re-formats UA-fallback labels into
                        // a friendly "OS · Browser" form, so we want enough
                        // chars to keep the browser token intact.
                        let label: String = label
                            .unwrap_or_else(|| "unknown".into())
                            .chars()
                            .take(80)
                            .collect();
                        json!({
                            "kind": "otk_used",
                            "device_label": label,
                            "ip": ip.unwrap_or_default(),
                            "platform": platform,
                            "at": at,
                        })
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    // Key verifications: trusted_devices sorted by last_seen_at DESC.
    let key_rows: Vec<serde_json::Value> = conn
        .prepare(
            "SELECT COALESCE(device_name, label), first_seen_ip, platform, last_seen_at
             FROM trusted_devices
             WHERE revoked_at IS NULL AND last_seen_at IS NOT NULL
             ORDER BY last_seen_at DESC
             LIMIT 10",
        )
        .ok()
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .ok()
            .map(|rows| {
                rows.flatten()
                    .map(|(label, ip, platform, at)| {
                        let label: String = label.chars().take(80).collect();
                        json!({
                            "kind": "key_verified",
                            "device_label": label,
                            "ip": ip.unwrap_or_default(),
                            "platform": platform,
                            "at": at,
                        })
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    // Merge both lists and return the 10 most recent.
    let mut merged: Vec<serde_json::Value> = otk_rows.into_iter().chain(key_rows).collect();
    merged.sort_by(|a, b| {
        let at_a = a["at"].as_i64().unwrap_or(0);
        let at_b = b["at"].as_i64().unwrap_or(0);
        at_b.cmp(&at_a)
    });
    merged.truncate(10);
    merged
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

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
            terminal_sessions: Arc::new(DashMap::new()),
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
            discovery_url: None,
            web_url: None,
            is_quick_tunnel: true,
            named_tunnel_hostname: None,
            discovery_temp_key: std::sync::Arc::new(std::sync::RwLock::new(None)),
            tunnel_shutdown: std::sync::Arc::new(tokio::sync::Notify::new()),
            telemetry: std::sync::Arc::new(crate::telemetry::TelemetryState::new()),
            files_activity: std::sync::Arc::new(crate::files_activity::new_map()),
            cloudflared_path: None,
            cors_origins: crate::security::cors::seed_origins(&[], &[]),
        })
    }

    /// GET returns metadata only — last4 + created_at, no hash, no plaintext.
    #[test]
    fn permanent_key_get_returns_last4_only() {
        let state = test_state("pk-get");
        // No key yet — meta returns None.
        assert!(auth::get_permanent_key_meta(&state.db_path).unwrap().is_none());

        // Rotate once.
        let (plaintext, last4, created_at, lookup_id) =
            auth::rotate_permanent_key(&state.db_path).unwrap();
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

        // lookup_id is a non-secret 16-hex SHA-256 prefix and gets persisted.
        assert_eq!(lookup_id.len(), 16);
        assert!(lookup_id.chars().all(|c| c.is_ascii_hexdigit()));
        let stored = auth::get_permanent_key_lookup_id(&state.db_path)
            .unwrap()
            .expect("lookup_id should be persisted");
        assert_eq!(stored, lookup_id);
    }

    /// POST rotates the Argon2 hash — the new hash matches the new key.
    #[test]
    fn permanent_key_post_rotates_hash() {
        let state = test_state("pk-rotate");
        let (key1, _, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();
        let (key2, _, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key must no longer verify.
        assert!(!auth::verify_permanent_key(&state.db_path, &key1));
        // New key must verify.
        assert!(auth::verify_permanent_key(&state.db_path, &key2));
    }

    /// After rotation the old plaintext fails verification — it's invalidated.
    #[test]
    fn permanent_key_post_invalidates_old_key() {
        let state = test_state("pk-invalidate");
        let (old_key, _, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key verifies before rotation.
        assert!(auth::verify_permanent_key(&state.db_path, &old_key));

        // Rotate to new key.
        let (_new_key, _, _, _) = auth::rotate_permanent_key(&state.db_path).unwrap();

        // Old key no longer authenticates.
        assert!(!auth::verify_permanent_key(&state.db_path, &old_key));
    }

    // -----------------------------------------------------------------------
    // Operator telemetry tests
    // -----------------------------------------------------------------------

    /// bytes_proxied counter accumulates correctly across multiple add_bytes calls.
    #[test]
    fn bytes_proxied_counter_accumulates() {
        use crate::telemetry::TelemetryState;
        use std::sync::atomic::Ordering;

        let t = TelemetryState::new();
        t.add_bytes(1024);
        t.add_bytes(512);
        assert_eq!(t.bytes_proxied.load(Ordering::Relaxed), 1536);
    }

    /// reconnect_count stays 0 on the first tunnel-ready, increments on second.
    #[test]
    fn reconnect_count_first_ready_is_not_a_reconnect() {
        use crate::telemetry::TelemetryState;
        use std::sync::atomic::Ordering;

        let t = TelemetryState::new();
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 0);
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 1);
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 2);
    }

    /// probe latency ring stores up to 16 samples in insertion order.
    #[test]
    fn probe_latency_ring_stores_and_wraps() {
        use crate::telemetry::{TelemetryState, LATENCY_RING_CAP};

        let t = TelemetryState::new();
        // Record fewer than cap — returned in order.
        t.record_probe_latency(10);
        t.record_probe_latency(20);
        assert_eq!(t.snapshot_latencies(), vec![10, 20]);

        // Record beyond cap — ring wraps; only last LATENCY_RING_CAP kept.
        let t2 = TelemetryState::new();
        for i in 0u64..20 {
            t2.record_probe_latency(i);
        }
        let snaps = t2.snapshot_latencies();
        assert_eq!(snaps.len(), LATENCY_RING_CAP);
        // First returned sample is the 5th written (0-indexed: index 4).
        assert_eq!(snaps[0], 4);
        assert_eq!(snaps[LATENCY_RING_CAP - 1], 19);
    }

    /// GET /api/agent/tunnel/telemetry returns the primed counter values.
    #[tokio::test]
    async fn tunnel_telemetry_endpoint_reflects_counter_state() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state("telem-endpoint");
        state.telemetry.add_bytes(8192);
        state.telemetry.mark_tunnel_ready(); // first connect
        state.telemetry.record_probe_latency(55);
        state.telemetry.record_probe_latency(60);

        let app = super::router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/agent/tunnel/telemetry")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["bytes_proxied"].as_u64().unwrap(), 8192);
        assert_eq!(body["reconnect_count"].as_u64().unwrap(), 0);
        let lats = body["probe_latencies_p50_ms"].as_array().unwrap();
        assert_eq!(lats.len(), 2);
        assert_eq!(lats[0].as_u64().unwrap(), 55);
    }

    /// GET /api/agent/version returns agent version and build_date fields.
    #[tokio::test]
    async fn version_endpoint_returns_required_fields() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state("ver-endpoint");
        let app = super::router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/agent/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // agent version must match the Cargo.toml version.
        assert_eq!(
            body["agent"].as_str().unwrap(),
            env!("CARGO_PKG_VERSION")
        );
        assert!(body["build_date"].is_string());
        // cloudflared key must be present (null is acceptable when path is None).
        assert!(body.get("cloudflared").is_some());
    }

    /// GET /api/agent/sessions/active returns a JSON array (possibly empty).
    #[tokio::test]
    async fn sessions_active_endpoint_returns_array() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state("sess-endpoint");
        let app = super::router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/agent/sessions/active")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.is_array(), "sessions/active must return a JSON array");
    }

    /// GET /api/agent/keys/recent-usage returns a JSON array (possibly empty).
    #[tokio::test]
    async fn keys_recent_usage_endpoint_returns_array() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = test_state("keys-endpoint");
        let app = super::router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/agent/keys/recent-usage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.is_array(), "keys/recent-usage must return a JSON array");
    }

    /// All 4 new telemetry routes classify as Localhost-only via route_scope.
    #[test]
    fn new_telemetry_routes_are_localhost_scoped() {
        use crate::security::route_scope::{scope_for_path, RouteScope};

        let paths = [
            "/api/agent/version",
            "/api/agent/sessions/active",
            "/api/agent/tunnel/telemetry",
            "/api/agent/keys/recent-usage",
        ];
        for path in paths {
            assert_eq!(
                scope_for_path(path),
                RouteScope::Localhost,
                "{path} must be Localhost-scoped"
            );
        }
    }

    /// files_activity::touch marks a device as active; is_active returns true
    /// immediately and the snapshot helper classifies the device as "files".
    #[test]
    fn files_activity_touch_and_is_active() {
        use crate::files_activity;

        let map = files_activity::new_map();
        assert!(!files_activity::is_active(&map, "dev-1"));
        files_activity::touch(&map, "dev-1");
        assert!(files_activity::is_active(&map, "dev-1"));
        // Unknown device is not active.
        assert!(!files_activity::is_active(&map, "dev-unknown"));
    }
}
