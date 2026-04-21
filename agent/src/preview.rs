use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::auth::require_auth;
use crate::AppState;

#[derive(Clone, Serialize)]
pub struct PreviewTarget {
    pub id: String,
    pub port: u16,
    pub label: String,
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

pub async fn api_previews_create(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<CreatePreviewReq>,
) -> impl IntoResponse {
    if require_auth(&state.signing_key, &jar).is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if body.port == 0 {
        return (StatusCode::BAD_REQUEST, "port required").into_response();
    }
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let label = body.label.unwrap_or_else(|| format!("localhost:{}", body.port));
    let target = PreviewTarget { id: id.clone(), port: body.port, label };
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
) -> impl IntoResponse {
    if require_auth(&state.signing_key, &jar).is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let targets: Vec<PreviewTarget> = state
        .preview_targets
        .iter()
        .map(|e| e.value().clone())
        .collect();
    Json(targets).into_response()
}

pub async fn api_previews_delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if require_auth(&state.signing_key, &jar).is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.preview_targets.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::NOT_FOUND, "preview not found").into_response(),
    }
}

pub async fn preview_proxy_handler(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path((id, rest)): Path<(String, String)>,
    req: Request,
) -> axum::response::Response {
    if require_auth(&state.signing_key, &jar).is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let target = match state.preview_targets.get(&id) {
        Some(t) => t.clone(),
        None => return (StatusCode::NOT_FOUND, "preview not found").into_response(),
    };

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

    preview_http(target, rest, req).await
}

async fn preview_http(target: PreviewTarget, rest: String, req: Request) -> axum::response::Response {
    let (parts, body) = req.into_parts();

    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let origin_url = format!("http://127.0.0.1:{}/{rest}{query}", target.port);

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();

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

    let upstream = client
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
