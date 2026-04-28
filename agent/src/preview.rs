use std::path::Path as StdPath;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::db::now_ts;
use crate::preview_token::{sign_share_token, verify_share_token, DEFAULT_SHARE_TTL_SECS};
use crate::AppState;

#[derive(Clone, Serialize)]
pub struct PreviewTarget {
    pub id: String,
    pub host_id: String,
    pub port: u16,
    pub label: String,
    pub created_at: i64,
}

#[derive(Clone, Serialize)]
pub struct PreviewHealth {
    pub status: String,
    pub checked_at: i64,
}

#[derive(Serialize)]
pub struct PreviewView {
    pub id: String,
    pub port: u16,
    pub label: String,
    pub created_at: i64,
    pub health: String,
    pub health_checked_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct CreatePreviewReq {
    pub port: u16,
    pub label: Option<String>,
}

#[derive(Serialize)]
pub struct CreatePreviewRes {
    pub id: String,
    pub path_prefix: String,
}

#[derive(Serialize)]
pub struct ShareUrlRes {
    pub url: String,
    pub expires_at: i64,
}

#[derive(Deserialize)]
pub struct ProxyQuery {
    #[serde(default)]
    pub t: Option<String>,
}

// === DB helpers ===

pub fn load_all_previews(db_path: &StdPath, host_id: &str) -> anyhow::Result<Vec<PreviewTarget>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, host_id, port, label, created_at FROM previews WHERE host_id = ?1",
    )?;
    let rows = stmt.query_map(params![host_id], |row| {
        Ok(PreviewTarget {
            id: row.get(0)?,
            host_id: row.get(1)?,
            port: row.get::<_, i64>(2)? as u16,
            label: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn insert_preview(db_path: &StdPath, target: &PreviewTarget) -> anyhow::Result<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO previews(id, host_id, port, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            target.id,
            target.host_id,
            target.port as i64,
            target.label,
            target.created_at
        ],
    )?;
    Ok(())
}

fn delete_preview(db_path: &StdPath, host_id: &str, id: &str) -> anyhow::Result<usize> {
    let conn = Connection::open(db_path)?;
    let affected = conn.execute(
        "DELETE FROM previews WHERE id = ?1 AND host_id = ?2",
        params![id, host_id],
    )?;
    Ok(affected)
}

// === Handlers ===

pub async fn api_previews_create(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Json(body): Json<CreatePreviewReq>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if body.port == 0 {
        return (StatusCode::BAD_REQUEST, "port required").into_response();
    }
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let label = body.label.unwrap_or_else(|| format!("localhost:{}", body.port));
    let target = PreviewTarget {
        id: id.clone(),
        host_id: state.host_info.host_id.clone(),
        port: body.port,
        label,
        created_at: now_ts(),
    };

    if let Err(e) = insert_preview(&state.db_path, &target) {
        // UNIQUE(host_id, port) collision is a user error, not a server error.
        let msg = e.to_string();
        if msg.contains("UNIQUE") {
            return (StatusCode::CONFLICT, "port already registered").into_response();
        }
        warn!(error=%e, "preview insert failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
    }
    state.preview_targets.insert(id.clone(), target);

    Json(CreatePreviewRes {
        path_prefix: format!("/preview/{id}"),
        id,
    })
    .into_response()
}

pub async fn api_previews_list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let host_id = state.host_info.host_id.clone();
    let mut out: Vec<PreviewView> = state
        .preview_targets
        .iter()
        .filter(|e| e.value().host_id == host_id)
        .map(|e| {
            let t = e.value();
            let h = state.preview_health.get(&t.id);
            let (status, checked_at) = match h.as_deref() {
                Some(ph) => (ph.status.clone(), Some(ph.checked_at)),
                None => ("unknown".to_string(), None),
            };
            PreviewView {
                id: t.id.clone(),
                port: t.port,
                label: t.label.clone(),
                created_at: t.created_at,
                health: status,
                health_checked_at: checked_at,
            }
        })
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Json(out).into_response()
}

pub async fn api_previews_delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let host_id = &state.host_info.host_id;
    let affected = delete_preview(&state.db_path, host_id, &id).unwrap_or(0);
    if affected == 0 {
        return (StatusCode::NOT_FOUND, "preview not found").into_response();
    }
    state.preview_targets.remove(&id);
    state.preview_health.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn api_previews_share(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let target = match state.preview_targets.get(&id) {
        Some(t) if t.host_id == state.host_info.host_id => t.clone(),
        _ => return (StatusCode::NOT_FOUND, "preview not found").into_response(),
    };
    let token = sign_share_token(
        &state.signing_key,
        &target.host_id,
        &target.id,
        DEFAULT_SHARE_TTL_SECS,
    );
    let expires_at = now_ts() + DEFAULT_SHARE_TTL_SECS;
    Json(ShareUrlRes {
        url: format!("/preview/{}?t={}", target.id, token),
        expires_at,
    })
    .into_response()
}

/// Route: `/preview/{id}` — delegates with empty rest so the share URL
/// `/preview/{id}/?t=<tok>` (empty trailing segment) hits the same logic.
pub async fn preview_proxy_root_handler(
    state: State<Arc<AppState>>,
    jar: CookieJar,
    Path(id): Path<String>,
    q: Query<ProxyQuery>,
    req: Request,
) -> axum::response::Response {
    preview_proxy_inner(state, jar, id, String::new(), q, req).await
}

pub async fn preview_proxy_handler(
    state: State<Arc<AppState>>,
    jar: CookieJar,
    Path((id, rest)): Path<(String, String)>,
    q: Query<ProxyQuery>,
    req: Request,
) -> axum::response::Response {
    preview_proxy_inner(state, jar, id, rest, q, req).await
}

async fn preview_proxy_inner(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    id: String,
    rest: String,
    Query(q): Query<ProxyQuery>,
    req: Request,
) -> axum::response::Response {
    let target = match state.preview_targets.get(&id) {
        Some(t) => t.clone(),
        None => return (StatusCode::NOT_FOUND, "preview not found").into_response(),
    };

    // Auth: accept a valid session cookie, a Bearer api_key, OR a `?t=` share token bound to (host_id, id).
    let bearer = crate::auth::extract_bearer(req.headers());
    let session_ok = crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_some();
    let token_ok = q
        .t
        .as_deref()
        .and_then(|tok| verify_share_token(&state.signing_key, tok))
        .is_some_and(|(host_id, preview_id)| {
            host_id == target.host_id && preview_id == target.id
        });
    if !session_ok && !token_ok {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let is_ws = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));

    if is_ws {
        let ws = match WebSocketUpgrade::from_request(req, &state).await {
            Ok(ws) => ws,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("ws upgrade: {e}")).into_response(),
        };
        return preview_ws_upgrade(ws, target, rest);
    }

    preview_http(state.clone(), target, rest, req).await
}

/// Rebuild the query string with the `t` share-token parameter removed. Prevents
/// the signed token from leaking into upstream access logs (vite, next dev, etc.).
fn strip_share_token(query: Option<&str>) -> String {
    let Some(q) = query else {
        return String::new();
    };
    let filtered: Vec<&str> = q
        .split('&')
        .filter(|p| !(p.starts_with("t=") || *p == "t"))
        .collect();
    if filtered.is_empty() {
        String::new()
    } else {
        format!("?{}", filtered.join("&"))
    }
}

async fn preview_http(
    state: Arc<AppState>,
    target: PreviewTarget,
    rest: String,
    req: Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();

    let query = strip_share_token(parts.uri.query());
    let origin_url = format!("http://127.0.0.1:{}/{rest}{query}", target.port);

    let mut headers = parts.headers.clone();
    headers.remove("host");
    headers.insert(
        "host",
        HeaderValue::from_str(&format!("localhost:{}", target.port)).unwrap(),
    );

    let body_bytes = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "body too large").into_response(),
    };

    let upstream = state
        .preview_client
        .request(parts.method, &origin_url)
        .headers(headers)
        .body(body_bytes)
        .send()
        .await;

    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => {
            warn!(error=%e, "preview proxy upstream error");
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_headers = upstream.headers().clone();

    let stream = upstream.bytes_stream();
    let body = Body::from_stream(stream);

    let mut response = axum::response::Response::builder().status(status);
    for (k, v) in resp_headers.iter() {
        if k == "transfer-encoding" {
            continue;
        }
        response = response.header(k, v);
    }
    response.body(body).unwrap().into_response()
}

fn preview_ws_upgrade(
    ws: WebSocketUpgrade,
    target: PreviewTarget,
    rest: String,
) -> axum::response::Response {
    ws.on_upgrade(move |client_ws| async move {
        if let Err(e) = pump_ws(target, rest, client_ws).await {
            warn!(error=%e, "preview ws proxy error");
        }
    })
}

async fn pump_ws(
    target: PreviewTarget,
    rest: String,
    client_ws: axum::extract::ws::WebSocket,
) -> anyhow::Result<()> {
    use axum::extract::ws::Message as AxumMsg;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TungMsg;

    let origin_url = format!("ws://127.0.0.1:{}/{rest}", target.port);
    let (origin_ws, _) = tokio_tungstenite::connect_async(&origin_url).await?;
    let (mut origin_tx, mut origin_rx) = origin_ws.split();
    let (mut client_tx, mut client_rx) = client_ws.split();

    let client_to_origin = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            let tung_msg = match msg {
                AxumMsg::Text(t) => TungMsg::Text(t.as_str().into()),
                AxumMsg::Binary(b) => TungMsg::Binary(b),
                AxumMsg::Ping(p) => TungMsg::Ping(p),
                AxumMsg::Pong(p) => TungMsg::Pong(p),
                AxumMsg::Close(_) => break,
            };
            if origin_tx.send(tung_msg).await.is_err() {
                break;
            }
        }
    };

    let origin_to_client = async {
        while let Some(Ok(msg)) = origin_rx.next().await {
            let axum_msg = match msg {
                TungMsg::Text(t) => AxumMsg::Text(t.as_str().into()),
                TungMsg::Binary(b) => AxumMsg::Binary(b),
                TungMsg::Ping(p) => AxumMsg::Ping(p),
                TungMsg::Pong(p) => AxumMsg::Pong(p),
                TungMsg::Close(_) => break,
                _ => continue,
            };
            if client_tx.send(axum_msg).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = client_to_origin => {},
        _ = origin_to_client => {},
    }

    Ok(())
}

// === Background health loop ===

pub fn spawn_health_loop(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let targets: Vec<(String, u16)> = state
                .preview_targets
                .iter()
                .map(|e| (e.value().id.clone(), e.value().port))
                .collect();
            for (id, port) in targets {
                let url = format!("http://127.0.0.1:{port}/");
                let resp = state
                    .http_client
                    .head(&url)
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await;
                let status = if resp.is_ok() { "ok" } else { "down" };
                state.preview_health.insert(
                    id,
                    PreviewHealth {
                        status: status.to_string(),
                        checked_at: now_ts(),
                    },
                );
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use axum::{extract::State, response::IntoResponse, Json};
    use axum_extra::extract::cookie::{Cookie, CookieJar};
    use dashmap::DashMap;
    use rusqlite::{params, Connection};

    use super::*;
    use crate::{
        auth::{insert_or_update_device, sign_session},
        db::{init_db, now_ts},
        local_sites,
        terminal_pty::TerminalSession,
    };

    fn test_state(name: &str) -> Arc<AppState> {
        let db_path = std::env::temp_dir().join(format!(
            "oxiremote-preview-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&db_path);
        init_db(&db_path).unwrap();

        let data_dir = db_path.parent().unwrap().join(format!("oxi-preview-{name}"));
        std::fs::create_dir_all(&data_dir).unwrap();

        Arc::new(AppState {
            db_path,
            signing_key: b"01234567890123456789012345678901".to_vec(),
            secure_cookies: false,
            terminal_sessions: Arc::new(DashMap::<String, Arc<TerminalSession>>::new()),
            preview_targets: DashMap::<String, PreviewTarget>::new(),
            preview_health: DashMap::new(),
            local_sites: local_sites::new_cache(),
            proxy_allowed_ports: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            pairing_attempts: DashMap::new(),
            workspace_root: PathBuf::from("."),
            host_info: crate::host::HostInfo {
                host_id: "test-host".into(),
                label: "test".into(),
                platform: "test".into(),
            },
            vapid_keys: Arc::new(crate::push::load_or_create_vapid(&data_dir).unwrap()),
            notify_token: "test-token".to_string(),
            http_client: reqwest::Client::new(),
            preview_client: reqwest::Client::new(),
            rate_limiter: Arc::new(crate::security::rate_limit::RateLimiter::new()),
            event_bus: crate::events::EventBus::new(),
            tunnel_url: Arc::new(std::sync::RwLock::new(None)),
            latest_tunnel_step: Arc::new(std::sync::RwLock::new(None)),
            recent_logs: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            desktop_available: false,
            desktop_service: None,
            discovery_url: None,
            discovery_temp_key: Arc::new(std::sync::RwLock::new(None)),
        })
    }

    #[test]
    fn strip_share_token_removes_t_only() {
        assert_eq!(strip_share_token(Some("t=abc")), "");
        assert_eq!(strip_share_token(Some("t=abc&x=1")), "?x=1");
        assert_eq!(strip_share_token(Some("x=1&t=abc&y=2")), "?x=1&y=2");
        assert_eq!(strip_share_token(Some("x=1")), "?x=1");
        assert_eq!(strip_share_token(None), "");
    }

    fn authed_jar(state: &AppState) -> CookieJar {
        let session_id = "session-preview";
        let device_id = "device-preview";
        let now = now_ts();
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params![session_id, now, device_id],
        )
        .unwrap();
        insert_or_update_device(&state.db_path, device_id, "Phone", None).unwrap();

        let token = sign_session(&state.signing_key, session_id);
        CookieJar::new().add(Cookie::new("oxiremote_session", token))
    }

    #[tokio::test]
    async fn preview_list_requires_auth() {
        let state = test_state("unauth");
        let response = api_previews_list(State(state), CookieJar::new(), axum::http::HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn preview_create_allows_active_auth() {
        let state = test_state("auth");
        let response = api_previews_create(
            State(state.clone()),
            authed_jar(state.as_ref()),
            axum::http::HeaderMap::new(),
            Json(CreatePreviewReq {
                port: 3000,
                label: Some("dev server".into()),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn preview_persists_across_load_all() {
        let state = test_state("persist");
        // insert via handler
        let _ = api_previews_create(
            State(state.clone()),
            authed_jar(state.as_ref()),
            axum::http::HeaderMap::new(),
            Json(CreatePreviewReq {
                port: 5173,
                label: None,
            }),
        )
        .await
        .into_response();

        let loaded = load_all_previews(&state.db_path, &state.host_info.host_id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].port, 5173);
        assert_eq!(loaded[0].host_id, "test-host");
    }

    #[tokio::test]
    async fn preview_delete_scoped_to_host() {
        let state = test_state("del");
        let jar = authed_jar(state.as_ref());
        let _ = api_previews_create(
            State(state.clone()),
            jar.clone(),
            axum::http::HeaderMap::new(),
            Json(CreatePreviewReq { port: 4000, label: None }),
        )
        .await
        .into_response();
        let id = load_all_previews(&state.db_path, &state.host_info.host_id).unwrap()[0]
            .id
            .clone();

        let resp = api_previews_delete(State(state.clone()), jar, axum::http::HeaderMap::new(), Path(id))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            load_all_previews(&state.db_path, &state.host_info.host_id)
                .unwrap()
                .len(),
            0
        );
    }
}
