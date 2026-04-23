use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path as AxumPath, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use rusqlite::{params, Connection};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::auth::require_active_auth;
use crate::db::now_ts;
use crate::terminal_pty::{WsIn, WsOut, WS_MAX_TEXT_BYTES};
use crate::AppState;

pub async fn api_terminal_session_ws(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    AxumPath(id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(owner_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let owned: anyhow::Result<bool> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT 1 FROM terminal_sessions WHERE terminal_session_id=?1 AND owner_session_id=?2",
        )?;
        Ok(stmt.exists(params![id, owner_session_id])?)
    })();

    match owned {
        Ok(true) => {
            if !state.terminal_sessions.contains_key(&id) {
                let _ = (|| -> anyhow::Result<()> {
                    let conn = Connection::open(&state.db_path)?;
                    let now = now_ts();
                    conn.execute(
                        "UPDATE terminal_sessions SET status='dead', last_seen_at=?2 WHERE terminal_session_id=?1 AND owner_session_id=?3",
                        params![id, now, owner_session_id],
                    )?;
                    Ok(())
                })();

                return (
                    StatusCode::GONE,
                    Json(serde_json::json!({ "error": "session not running" })),
                )
                    .into_response();
            }

            let state2 = state.clone();
            ws.on_upgrade(move |socket| handle_terminal_ws(state2, id, owner_session_id, socket))
        }
        Ok(false) => StatusCode::FORBIDDEN.into_response(),
        Err(err) => {
            warn!(error=%err, "terminal ws ownership check failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn handle_terminal_ws(
    state: Arc<AppState>,
    id: String,
    owner_session_id: String,
    mut socket: WebSocket,
) {
    let sess = match state.terminal_sessions.get(&id) {
        Some(s) => s.clone(),
        None => {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    if sess.owner_session_id != owner_session_id {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    let mut rx = sess.output_tx.subscribe();
    let mut attached = false;

    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break; };
                match msg {
                    Message::Text(text) => {
                        if text.len() > WS_MAX_TEXT_BYTES { break; }
                        let Ok(frame) = serde_json::from_str::<WsIn>(&text) else { continue };
                        match frame {
                            WsIn::Attach { last_seq } => {
                                if attached { continue; }
                                attached = true;
                                let snap = sess.snapshot_since(last_seq);
                                let frame = WsOut::Snapshot {
                                    from_seq: snap.from_seq,
                                    to_seq: snap.to_seq,
                                    data: snap.data,
                                };
                                if let Ok(text) = serde_json::to_string(&frame) {
                                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                                }
                            }
                            WsIn::Input { data } => {
                                if let Ok(mut w) = sess.writer.lock() {
                                    use std::io::Write;
                                    let _ = w.write_all(data.as_bytes());
                                    let _ = w.flush();
                                }
                            }
                            WsIn::Ping => {}
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            out = rx.recv() => {
                match out {
                    Ok(frame) => {
                        // Skip live chunks until the client has attached, so replay isn't duplicated.
                        if !attached && matches!(frame, WsOut::Chunk { .. }) { continue; }
                        if let Ok(text) = serde_json::to_string(&frame) {
                            if socket.send(Message::Text(text.into())).await.is_err() { break; }
                        }
                        if matches!(frame, WsOut::Exit { .. }) {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Subscriber fell behind the 1024-slot broadcast buffer.
                        // Drop this subscriber; PTY keeps running for others.
                        warn!(session = %id, "ws subscriber lagged, dropping");
                        break;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
}
