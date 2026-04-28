// HTTP routes for Web Push subscribe / deliver.
//
// Public surface:
//   GET    /api/push/vapid-public  — no auth, just the public key
//   POST   /api/push/subscribe     — cookie auth; persists endpoint+keys for this device
//   DELETE /api/push/subscribe     — cookie auth; removes this device's subscription
//   POST   /api/notify             — Bearer notify-token auth; fans out to all subs for the host

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::db::now_ts;
use crate::push::{self, PushSubscription};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/push/vapid-public", get(vapid_public))
        .route(
            "/api/push/subscribe",
            post(subscribe).delete(unsubscribe),
        )
        .route("/api/notify", post(notify))
}

#[derive(Serialize)]
struct VapidPublicResponse {
    public_key: String,
}

async fn vapid_public(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(VapidPublicResponse {
        public_key: state.vapid_keys.public_b64url.clone(),
    })
}

#[derive(Deserialize)]
struct SubscribeRequest {
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// Returns `device_id` for an authenticated session, or None.
fn device_id_for_session(db_path: &std::path::PathBuf, session_id: &str) -> Option<String> {
    let conn = Connection::open(db_path).ok()?;
    conn.query_row(
        "SELECT device_id FROM sessions WHERE session_id = ?1",
        params![session_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

async fn subscribe(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(body): Json<SubscribeRequest>,
) -> impl IntoResponse {
    // For Bearer path, require_tunnel_auth returns device_id directly.
    // For cookie path, resolve device_id via session_id lookup.
    let bearer = crate::auth::extract_bearer(&headers);
    let device_id = if let Some(ref b) = bearer {
        match crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, Some(b.as_str())).await {
            Some(did) => did,
            None => return StatusCode::UNAUTHORIZED.into_response(),
        }
    } else {
        let Some(session_id) = crate::auth::require_active_auth(&state.db_path, &state.signing_key, &jar) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        let Some(did) = device_id_for_session(&state.db_path, &session_id) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        did
    };

    if body.endpoint.is_empty() || body.p256dh.is_empty() || body.auth.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing fields").into_response();
    }
    if !body.endpoint.starts_with("https://") {
        return (StatusCode::BAD_REQUEST, "endpoint must be https").into_response();
    }

    let Ok(conn) = Connection::open(&state.db_path) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let host_id = state.host_info.host_id.clone();

    // Stale-sub cleanup: an FCM endpoint uniquely identifies a browser's push
    // subscription. If a different device_id on this host ever owned this
    // endpoint (re-pairing mints a fresh device_id), its row is stale — drop
    // it so we don't waste a send on every notify.
    let _ = conn.execute(
        "DELETE FROM push_subscriptions
         WHERE host_id=?1 AND endpoint=?2 AND device_id != ?3",
        params![host_id, body.endpoint, device_id],
    );

    let res = conn.execute(
        "INSERT INTO push_subscriptions(host_id, device_id, endpoint, p256dh, auth, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(host_id, device_id) DO UPDATE SET
           endpoint=excluded.endpoint,
           p256dh=excluded.p256dh,
           auth=excluded.auth,
           created_at=excluded.created_at",
        params![
            host_id,
            device_id,
            body.endpoint,
            body.p256dh,
            body.auth,
            now_ts()
        ],
    );

    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(error=%e, "push subscribe insert failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn unsubscribe(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
) -> impl IntoResponse {
    // For Bearer path, require_tunnel_auth returns device_id directly.
    // For cookie path, resolve device_id via session_id lookup.
    let bearer = crate::auth::extract_bearer(&headers);
    let device_id = if let Some(ref b) = bearer {
        match crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, Some(b.as_str())).await {
            Some(did) => did,
            None => return StatusCode::UNAUTHORIZED.into_response(),
        }
    } else {
        let Some(session_id) = crate::auth::require_active_auth(&state.db_path, &state.signing_key, &jar) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        let Some(did) = device_id_for_session(&state.db_path, &session_id) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        did
    };

    let Ok(conn) = Connection::open(&state.db_path) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let _ = conn.execute(
        "DELETE FROM push_subscriptions WHERE host_id=?1 AND device_id=?2",
        params![state.host_info.host_id, device_id],
    );
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct NotifyRequest {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    deep_link: Option<String>,
    #[serde(default)]
    urgency: Option<String>,
    #[serde(default)]
    ttl: Option<u32>,
    /// Optional target device; if omitted, fans out to all devices on this host.
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Serialize)]
struct NotifyResponse {
    dispatched: usize,
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(str::trim).map(str::to_string)
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

async fn notify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NotifyRequest>,
) -> impl IntoResponse {
    let Some(token) = bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer").into_response();
    };
    if !constant_time_eq(&token, &state.notify_token) {
        return (StatusCode::UNAUTHORIZED, "bad bearer").into_response();
    }

    if body.title.is_empty() {
        return (StatusCode::BAD_REQUEST, "title required").into_response();
    }
    if let Some(link) = body.deep_link.as_deref()
        && (!link.starts_with('/') || link.starts_with("//")) {
            return (StatusCode::BAD_REQUEST, "deep_link must be own-origin absolute path").into_response();
        }
    // RFC 8030 §5.3: urgency must be one of four fixed values.
    let urgency = match body.urgency.as_deref().unwrap_or("normal") {
        v @ ("very-low" | "low" | "normal" | "high") => v.to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "urgency must be very-low|low|normal|high",
            )
                .into_response();
        }
    };

    let subs = match load_subscriptions(&state.db_path, &state.host_info.host_id, body.device_id.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            warn!(error=%e, "load subscriptions");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if subs.is_empty() {
        return Json(NotifyResponse { dispatched: 0 }).into_response();
    }

    let payload_json = serde_json::json!({
        "title": body.title,
        "body": body.body,
        "deep_link": body.deep_link,
    })
    .to_string();

    let ttl = body.ttl.unwrap_or(60);

    // Fan-out asynchronously. Sends are serial within the task; fine for a
    // personal agent (N devices < ~10). If this grows to team-scale, switch
    // to `FuturesUnordered` for concurrent sends.
    let state_for_task = state.clone();
    let payload_bytes = payload_json.into_bytes();
    let dispatched = subs.len();
    tokio::spawn(async move {
        for (device_id, sub) in subs {
            let res = push::send_push(
                &state_for_task.http_client,
                &state_for_task.vapid_keys,
                &sub,
                "mailto:push@oxiremote.local",
                &payload_bytes,
                ttl,
                &urgency,
            )
            .await;

            match res {
                Ok(r) if r.gone => {
                    info!(device_id = %device_id, status = r.status, "push gone — removing subscription");
                    if let Ok(conn) = Connection::open(&state_for_task.db_path) {
                        let _ = conn.execute(
                            "DELETE FROM push_subscriptions WHERE host_id=?1 AND device_id=?2",
                            params![state_for_task.host_info.host_id, device_id],
                        );
                    }
                }
                Ok(r) if r.status >= 400 => {
                    warn!(device_id = %device_id, status = r.status, "push failed");
                }
                Ok(_) => {}
                Err(e) => warn!(device_id = %device_id, error=%e, "push send error"),
            }
        }
    });

    Json(NotifyResponse { dispatched }).into_response()
}

fn load_subscriptions(
    db_path: &std::path::PathBuf,
    host_id: &str,
    device_filter: Option<&str>,
) -> anyhow::Result<Vec<(String, PushSubscription)>> {
    let conn = Connection::open(db_path)?;
    let mut out = Vec::new();
    if let Some(device_id) = device_filter {
        let mut stmt = conn.prepare(
            "SELECT device_id, endpoint, p256dh, auth
             FROM push_subscriptions
             WHERE host_id=?1 AND device_id=?2",
        )?;
        let rows = stmt.query_map(params![host_id, device_id], row_to_sub)?;
        for r in rows {
            out.push(r?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT device_id, endpoint, p256dh, auth
             FROM push_subscriptions
             WHERE host_id=?1",
        )?;
        let rows = stmt.query_map(params![host_id], row_to_sub)?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}

fn row_to_sub(row: &rusqlite::Row) -> rusqlite::Result<(String, PushSubscription)> {
    Ok((
        row.get::<_, String>(0)?,
        PushSubscription {
            endpoint: row.get(1)?,
            p256dh_b64: row.get(2)?,
            auth_b64: row.get(3)?,
        },
    ))
}
