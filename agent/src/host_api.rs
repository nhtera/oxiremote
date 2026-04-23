use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use axum::routing::get;
use axum_extra::extract::cookie::CookieJar;

use crate::auth::require_active_auth;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/host", get(api_host_info))
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
