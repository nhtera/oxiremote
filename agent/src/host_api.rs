use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use axum::routing::get;
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::auth::require_active_auth;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/host", get(api_host_info))
        .route("/api/hosts/{id}/desktop/capabilities", get(api_desktop_capabilities))
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

        let body = json!({
            "available": available,
            "quality_tiers": ["low", "med", "high"],
            "monitors": monitors,
        });
        return (StatusCode::OK, Json(body)).into_response();
    }

    #[cfg(not(feature = "desktop"))]
    {
        let body = json!({
            "available": false,
            "quality_tiers": ["low", "med", "high"],
            "monitors": [],
        });
        (StatusCode::OK, Json(body)).into_response()
    }
}
