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

use crate::auth::require_active_auth;
use crate::events::AgentEvent;
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
    Path(device_id): Path<String>,
    Json(body): Json<DeviceRenameBody>,
) -> impl IntoResponse {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
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
) -> impl IntoResponse {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (StatusCode::OK, Json(state.host_info.clone())).into_response()
}

/// GET /api/hosts/{id}/desktop/capabilities
///
/// Returns desktop availability, supported quality tiers, and monitor list.
/// Response is safe to call at any time — no capture is performed.
pub async fn api_desktop_capabilities(
    State(state): State<Arc<AppState>>,
    Path(host_id): Path<String>,
    jar: CookieJar,
) -> impl IntoResponse {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Guard against future multi-host routing: only serve this agent's own id.
    if host_id != state.host_info.host_id {
        return StatusCode::NOT_FOUND.into_response();
    }

    #[cfg(feature = "desktop")]
    {
        let available = state.desktop_available;
        let monitors: Vec<serde_json::Value> = if available {
            desktop::list_monitors()
                .into_iter()
                .map(|m| json!({ "id": m.id, "label": m.label, "width": m.width, "height": m.height }))
                .collect()
        } else {
            vec![]
        };

        // Surface the operator's pipeline preference so the SPA can mount the
        // correct hook (JPEG vs H.264) before opening the WS — spares a
        // doomed H.264 handshake when the server will choose JPEG anyway.
        let preferred_pipeline = crate::pipeline_selection::operator_preference().wire_name();

        let body = json!({
            "available": available,
            "quality_tiers": ["low", "med", "high"],
            "monitors": monitors,
            "preferred_pipeline": preferred_pipeline,
        });
        (StatusCode::OK, Json(body)).into_response()
    }

    #[cfg(not(feature = "desktop"))]
    {
        let body = json!({
            "available": false,
            "quality_tiers": ["low", "med", "high"],
            "monitors": [],
            "preferred_pipeline": "jpeg",
        });
        (StatusCode::OK, Json(body)).into_response()
    }
}
