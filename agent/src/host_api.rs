use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use axum::routing::{get, patch};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::events::AgentEvent;
use crate::env_defaults;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/host", get(api_host_info))
        .route("/api/hosts/{id}/desktop/capabilities", get(api_desktop_capabilities))
        .route("/api/devices/{id}", patch(api_device_rename))
}

#[derive(Deserialize)]
struct DeviceRenameBody {
    name: Option<String>,
}

/// PATCH /api/devices/{id} — tunnel-scoped device rename.
async fn api_device_rename(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(device_id): Path<String>,
    Json(body): Json<DeviceRenameBody>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let name_ref: Option<&str> = body.name.as_deref();
    match crate::auth::rename_device(&state.db_path, &device_id, name_ref) {
        Ok(()) => {
            info!(device_id = %device_id, "device renamed via host api");
            // DeviceApproved is the closest existing event that causes the SPA
            // Devices page to refresh a device row.
            state.event_bus.send(AgentEvent::DeviceApproved { device_id });
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, device_id=%device_id, "rename device via host api failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

async fn api_host_info(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // `discovery_configured` is true when the discovery worker URL is active
    // (env set non-empty or bundled default in use). False only when the user
    // explicitly cleared OXI_DISCOVERY_URL= to opt out.
    let discovery_configured = env_defaults::discovery_url().is_some();
    // `tunnel_mode`: "named" when ~/.config/oxiremote/tunnel.toml was present
    // at startup; "quick" otherwise. Named tunnels have stable hostnames and
    // do not need the discovery banner.
    let tunnel_mode = if state.is_quick_tunnel { "quick" } else { "named" };
    // `tunnel_named_hostname`: the hostname from tunnel.toml when running in
    // named-tunnel mode. The SPA's URL allowlist auto-accepts this domain so
    // named-tunnel users don't need to configure anything. Null for Quick Tunnel.
    let tunnel_named_hostname: serde_json::Value = state
        .named_tunnel_hostname
        .as_deref()
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    (StatusCode::OK, Json(json!({
        "host_id": state.host_info.host_id,
        "label": state.host_info.label,
        "platform": state.host_info.platform,
        "discovery_configured": discovery_configured,
        "tunnel_mode": tunnel_mode,
        "tunnel_named_hostname": tunnel_named_hostname,
    }))).into_response()
}

/// GET /api/hosts/{id}/desktop/capabilities
///
/// Returns desktop availability, supported quality tiers, and monitor list.
/// Response is safe to call at any time — no capture is performed.
pub async fn api_desktop_capabilities(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(host_id): Path<String>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Guard against future multi-host routing: only serve this agent's own id.
    if host_id != state.host_info.host_id {
        return StatusCode::NOT_FOUND.into_response();
    }

    #[cfg(feature = "desktop")]
    {
        use crate::pipeline_selection::{operator_preference, ClientCapabilities, OperatorPref};

        let available = state.desktop_available;
        let monitors: Vec<serde_json::Value> = if available {
            desktop::list_monitors()
                .into_iter()
                .map(|m| json!({ "id": m.id, "label": m.label, "width": m.width, "height": m.height }))
                .collect()
        } else {
            vec![]
        };

        // Best-effort hint for the SPA so it can pick the right session hook
        // before the WS upgrade. We don't know the real client capabilities
        // here (that handshake happens after upgrade), so we feed `choose()`
        // an optimistic capability set: WebCodecs+H.264 baseline. That gives
        // the SPA the "what would Auto resolve to right now" answer, which is
        // what the pill tooltip references as `chosen_default`.
        let op = operator_preference();
        let optimistic_caps = ClientCapabilities {
            codecs: vec!["h264-baseline-3.1".to_string()],
            webcodecs: true,
        };
        let (chosen_default, default_reason) =
            match crate::pipeline_selection::choose(op, &optimistic_caps) {
                Ok(d) => (d.pipeline.wire_name(), d.reason),
                // `forced-h264-no-client` is unreachable with the optimistic
                // caps above; if a future refactor makes it reachable we'd
                // rather surface `h264` + an explanatory reason than 500.
                Err(e) => ("h264", e.reason),
            };

        // `available_pipelines` is the *transport list* the binary can speak
        // at all (build-time feature gate). `preferred_pipeline` is what the
        // operator's env var picks; `chosen_default` is what `Auto` would
        // resolve to for the SPA right now.
        let mut available_pipelines = vec!["jpeg".to_string()];
        if crate::pipeline_selection::H264_COMPILED {
            available_pipelines.push("h264".to_string());
        }
        let preferred_pipeline = match op {
            OperatorPref::Jpeg => "jpeg",
            OperatorPref::Auto => "auto",
            #[cfg(feature = "h264")]
            OperatorPref::H264 => "h264",
        };

        let body = json!({
            "available": available,
            "quality_tiers": ["low", "med", "high"],
            "monitors": monitors,
            "available_pipelines": available_pipelines,
            "preferred_pipeline": preferred_pipeline,
            "chosen_default": chosen_default,
            "default_reason": default_reason,
        });
        (StatusCode::OK, Json(body)).into_response()
    }

    #[cfg(not(feature = "desktop"))]
    {
        let body = json!({
            "available": false,
            "quality_tiers": ["low", "med", "high"],
            "monitors": [],
            "available_pipelines": ["jpeg"],
            "preferred_pipeline": "jpeg",
            "chosen_default": "jpeg",
            "default_reason": "no-desktop-feature",
        });
        (StatusCode::OK, Json(body)).into_response()
    }
}
