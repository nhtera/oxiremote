// Opt-in localhost-port reverse proxy mounted at `/proxy/{port}/...`.
//
// Goal: a phone hits `<tunnel>/proxy/3000/` and lands on the laptop's Vite dev
// server with HMR (WS) and SSE intact. Distinct from `/preview/{id}/...` —
// previews are saved+labelled+shareable; this proxy is just "open the port".
//
// Security model:
//   - Default deny — every port must be flipped on via `/api/agent/proxy/ports`.
//   - Auth = session cookie. Tunnel-side requests bypass `api_key_guard` (path
//     prefix exempt) and we re-validate the cookie inside the handler.
//   - Loopback callers hit the handler directly (no auth) — same convention as
//     the rest of the codebase; the tunnel is the only adversary surface.
//   - Set-Cookie Path attribute is rewritten so upstream cookies stay scoped to
//     `/proxy/<port>/` and don't bleed across ports.
//   - Our `oxiremote_session` cookie is stripped before forwarding upstream so
//     the proxied app can't read or echo it back.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::CookieJar;
use tracing::warn;

use crate::security::route_scope::is_tunnel_request;
use crate::AppState;

/// `/proxy/{port}` — 308 to `/proxy/{port}/` so relative paths in upstream
/// HTML (`<script src="main.js">`) resolve as `/proxy/{port}/main.js` rather
/// than `/proxy/main.js`.
pub async fn proxy_root_redirect(
    Path(port_str): Path<String>,
    req: Request,
) -> axum::response::Response {
    let target = match req.uri().query() {
        Some(q) => format!("/proxy/{port_str}/?{q}"),
        None => format!("/proxy/{port_str}/"),
    };
    axum::response::Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, target)
        .body(Body::empty())
        .unwrap()
}

pub async fn proxy_root_handler(
    state: State<Arc<AppState>>,
    jar: CookieJar,
    Path(port_str): Path<String>,
    req: Request,
) -> axum::response::Response {
    proxy_dispatch(state, jar, port_str, String::new(), req).await
}

pub async fn proxy_handler(
    state: State<Arc<AppState>>,
    jar: CookieJar,
    Path((port_str, rest)): Path<(String, String)>,
    req: Request,
) -> axum::response::Response {
    proxy_dispatch(state, jar, port_str, rest, req).await
}

async fn proxy_dispatch(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    port_str: String,
    rest: String,
    req: Request,
) -> axum::response::Response {
    let Ok(port) = port_str.parse::<u16>() else {
        return (StatusCode::BAD_REQUEST, "invalid port").into_response();
    };

    if !state.proxy_allowed_ports.read().unwrap().contains(&port) {
        return (StatusCode::FORBIDDEN, "port not enabled").into_response();
    }

    // Tunnel-side callers must have a valid session cookie or Bearer api_key.
    // Loopback callers are trusted (they're already on the host).
    let bearer = crate::auth::extract_bearer(req.headers());
    if is_tunnel_request(req.headers())
        && crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none()
    {
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
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("ws upgrade: {e}")).into_response()
            }
        };
        return proxy_ws_upgrade(ws, port, rest);
    }

    proxy_http(state.clone(), port, rest, req).await
}

async fn proxy_http(
    state: Arc<AppState>,
    port: u16,
    rest: String,
    req: Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();

    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let origin_url = format!("http://127.0.0.1:{port}/{rest}{query}");

    let mut headers = parts.headers.clone();
    headers.remove(header::HOST);
    headers.insert(
        header::HOST,
        HeaderValue::from_str(&format!("localhost:{port}")).unwrap(),
    );
    strip_self_session_cookie(&mut headers);

    // Vite dev servers and most local apps ship small bundles; 10 MiB matches
    // the preview proxy's existing cap.
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
            warn!(error=%e, port, "proxy upstream error");
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_headers = upstream.headers().clone();
    rewrite_set_cookie_paths(&mut resp_headers, port);

    let stream = upstream.bytes_stream();
    let body = Body::from_stream(stream);

    let mut response = axum::response::Response::builder().status(status);
    for (k, v) in resp_headers.iter() {
        // hop-by-hop headers and content-length must not be re-emitted; the
        // streamed body's length is unknown to us.
        if k == header::TRANSFER_ENCODING || k == header::CONTENT_LENGTH {
            continue;
        }
        response = response.header(k, v);
    }
    response.body(body).unwrap().into_response()
}

fn proxy_ws_upgrade(
    ws: WebSocketUpgrade,
    port: u16,
    rest: String,
) -> axum::response::Response {
    ws.on_upgrade(move |client_ws| async move {
        if let Err(e) = pump_ws(port, rest, client_ws).await {
            warn!(error=%e, port, "proxy ws error");
        }
    })
}

async fn pump_ws(
    port: u16,
    rest: String,
    client_ws: axum::extract::ws::WebSocket,
) -> anyhow::Result<()> {
    use axum::extract::ws::Message as AxumMsg;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TungMsg;

    let origin_url = format!("ws://127.0.0.1:{port}/{rest}");
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

/// Drop our `oxiremote_session` cookie from the outgoing `Cookie:` header so
/// the proxied app can't read it. Other cookies (e.g. ones the app itself set
/// previously) pass through untouched.
fn strip_self_session_cookie(headers: &mut HeaderMap) {
    let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
        return;
    };
    let kept: Vec<&str> = raw
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.starts_with("oxiremote_session="))
        .collect();
    if kept.is_empty() {
        headers.remove(header::COOKIE);
    } else if let Ok(value) = HeaderValue::from_str(&kept.join("; ")) {
        headers.insert(header::COOKIE, value);
    }
}

/// Rewrite `Path=...` on every `Set-Cookie` header so cookies the upstream app
/// sets land scoped to `/proxy/<port>/` instead of `/`. Without this, port 3000
/// and port 5173 cookies would collide on the host's origin.
fn rewrite_set_cookie_paths(headers: &mut HeaderMap, port: u16) {
    let prefix = format!("/proxy/{port}");

    let cookies: Vec<HeaderValue> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .cloned()
        .collect();
    if cookies.is_empty() {
        return;
    }
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        let Ok(value) = cookie.to_str() else {
            // Non-ASCII set-cookie shouldn't happen for sane upstreams; pass
            // through unmodified rather than dropping the header.
            headers.append(header::SET_COOKIE, cookie);
            continue;
        };
        let rewritten = rewrite_cookie_path(value, &prefix);
        if let Ok(hv) = HeaderValue::from_str(&rewritten) {
            headers.append(header::SET_COOKIE, hv);
        }
    }
}

fn rewrite_cookie_path(cookie: &str, prefix: &str) -> String {
    // Iterate attributes preserving order. If a `Path=` attribute exists,
    // replace its value; otherwise append `Path=<prefix>/`.
    let mut found_path = false;
    let mut parts: Vec<String> = cookie
        .split(';')
        .map(|seg| {
            let trimmed = seg.trim();
            if let Some(rest) = trimmed.strip_prefix("Path=").or_else(|| trimmed.strip_prefix("path=")) {
                found_path = true;
                let new_path = scope_path(rest, prefix);
                format!("Path={new_path}")
            } else {
                trimmed.to_string()
            }
        })
        .collect();
    if !found_path {
        parts.push(format!("Path={prefix}/"));
    }
    parts.join("; ")
}

fn scope_path(upstream_path: &str, prefix: &str) -> String {
    // Upstream paths come in three flavours:
    //   "/"        → "/proxy/<port>/"
    //   "/foo"     → "/proxy/<port>/foo"
    //   ""/missing → "/proxy/<port>/"
    let trimmed = upstream_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        format!("{prefix}/")
    } else if let Some(suffix) = trimmed.strip_prefix('/') {
        format!("{prefix}/{suffix}")
    } else {
        // Non-absolute path values are technically invalid per RFC 6265 but
        // some libraries emit them — preserve as-is under our prefix.
        format!("{prefix}/{trimmed}")
    }
}

// Helper used by the agent API to mutate the in-memory allowlist atomically
// with the persisted setting.
pub fn set_allowed(state: &AppState, port: u16, enabled: bool) -> anyhow::Result<Vec<u16>> {
    let mut guard = state.proxy_allowed_ports.write().unwrap();
    if enabled {
        guard.insert(port);
    } else {
        guard.remove(&port);
    }
    let mut sorted: Vec<u16> = guard.iter().copied().collect();
    sorted.sort_unstable();
    crate::db::save_proxy_allowed_ports(&state.db_path, &sorted)?;
    Ok(sorted)
}

// Silences the `Uri` unused import lint when no other module needs it; keeps
// the file self-contained for future tweaks.
#[allow(dead_code)]
fn _uri_marker(_: Uri) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_set_cookie_with_root_path() {
        let out = rewrite_cookie_path("sid=abc; Path=/; HttpOnly", "/proxy/3000");
        assert!(out.contains("Path=/proxy/3000/"));
        assert!(out.contains("HttpOnly"));
        assert!(out.starts_with("sid=abc"));
    }

    #[test]
    fn rewrites_set_cookie_with_subpath() {
        let out = rewrite_cookie_path("token=xyz; Path=/api; Secure", "/proxy/5173");
        assert!(out.contains("Path=/proxy/5173/api"));
        assert!(out.contains("Secure"));
    }

    #[test]
    fn appends_path_when_missing() {
        let out = rewrite_cookie_path("k=v; Secure", "/proxy/3000");
        assert!(out.ends_with("; Path=/proxy/3000/"));
    }

    #[test]
    fn case_insensitive_path_attr() {
        // RFC says attribute names are case-insensitive; some upstreams send "path=".
        let out = rewrite_cookie_path("k=v; path=/admin", "/proxy/8080");
        assert!(out.contains("Path=/proxy/8080/admin"));
    }

    #[test]
    fn strips_oxiremote_session_only() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("oxiremote_session=abc; theme=dark; sid=42"),
        );
        strip_self_session_cookie(&mut headers);
        let kept = headers.get(header::COOKIE).unwrap().to_str().unwrap();
        assert!(!kept.contains("oxiremote_session"));
        assert!(kept.contains("theme=dark"));
        assert!(kept.contains("sid=42"));
    }

    #[test]
    fn removes_cookie_header_when_only_self() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("oxiremote_session=abc"),
        );
        strip_self_session_cookie(&mut headers);
        assert!(headers.get(header::COOKIE).is_none());
    }
}
