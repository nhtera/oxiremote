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
        .route("/api/agent/qr", get(api_agent_qr))
        .route("/api/agent/keys/one-time", post(api_agent_keys_one_time))
        .route("/api/agent/approvals/pending", get(api_agent_approvals_pending))
        .route("/api/agent/approvals/{id}/approve", post(api_agent_approve))
        .route("/api/agent/approvals/{id}/reject", post(api_agent_reject))
        .route("/api/agent/settings/auto-approve", post(api_agent_settings_auto_approve))
        .route(
            "/api/agent/proxy/ports",
            get(api_agent_proxy_ports_list).post(api_agent_proxy_ports_set),
        )
}

async fn api_agent_state(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let tunnel_url = state
        .tunnel_url
        .read()
        .ok()
        .and_then(|g| g.clone());
    let connected_devices = state.terminal_sessions.len();
    Json(json!({
        "tunnel_url": tunnel_url,
        "host_id": state.host_info.host_id,
        "label": state.host_info.label,
        "platform": state.host_info.platform,
        "connected_devices": connected_devices,
    }))
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
