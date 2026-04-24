// Localhost-only API under `/api/agent/*`. Surfaces internal agent state and
// event stream to the in-process TUI, system tray, and `/agent` dashboard.
// Route scope enforces tunnel 403 — no auth on these handlers.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
    routing::get,
};
use futures_util::stream::Stream;
use qrcode::{QrCode, render::svg};
use serde_json::json;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

use crate::AppState;
use crate::events::AgentEvent;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/agent/events", get(api_agent_events))
        .route("/api/agent/state", get(api_agent_state))
        .route("/api/agent/qr", get(api_agent_qr))
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

async fn api_agent_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_bus.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(event) => Some(Ok(event_to_sse(&event))),
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
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
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
