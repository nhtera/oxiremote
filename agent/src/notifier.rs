// Desktop notifications via `notify-rust` + Web Push for agent-end events.
// Desktop toasts are best-effort and silently no-op on headless/sandboxed hosts.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::events::{AgentEvent, EventBus};
use crate::push::{send_agent_finished_push, AgentFinishedPushParams, PushSubscription, VapidKeys};

const APP_NAME: &str = "OxiRemote";

/// One-shot launch toast. Best-effort; failures (no DBus, sandboxed, etc.)
/// are dropped — startup must never block on a notification daemon.
pub fn show_startup(addr: SocketAddr) {
    let body = format!("Open http://{addr}/agent for the host dashboard.");
    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .appname(APP_NAME)
            .summary("OxiRemote running")
            .body(&body)
            .timeout(notify_rust::Timeout::Milliseconds(4_000))
            .show();
    });
}

/// Read the notify toggle and push subscriptions for a session from SQLite.
/// Returns `None` when the toggle is off or the session row is not found.
fn load_agent_push_targets(
    db_path: &PathBuf,
    host_id: &str,
    session_id: &str,
) -> anyhow::Result<Option<Vec<PushSubscription>>> {
    let conn = Connection::open(db_path)?;

    let notify_enabled: bool = conn
        .query_row(
            "SELECT notify_on_agent_end FROM terminal_sessions WHERE terminal_session_id=?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false);

    if !notify_enabled {
        return Ok(None);
    }

    let mut stmt = conn.prepare(
        "SELECT endpoint, p256dh, auth FROM push_subscriptions WHERE host_id=?1",
    )?;
    let subs: Vec<PushSubscription> = stmt
        .query_map(params![host_id], |row| {
            Ok(PushSubscription {
                endpoint: row.get(0)?,
                p256dh_b64: row.get(1)?,
                auth_b64: row.get(2)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Some(subs))
}

/// Subscribe to the event bus and surface device-pending and tunnel-down events
/// as desktop notifications. Throttles per-key (device or "tunnel") so
/// a flapping client/process cannot spam.
///
/// Also handles `SessionAgentEnded` — when the session has `notify_on_agent_end`
/// set, fires a Web Push to all registered push subscriptions for the host.
pub fn spawn_event_notifier(
    bus: Arc<EventBus>,
    db_path: PathBuf,
    host_id: String,
    vapid_keys: Arc<VapidKeys>,
    http_client: reqwest::Client,
    notify_subject: String,
) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        // (dedup_key, last_fired_instant) — single-slot; one category at a time.
        let mut last_fired: Option<(String, std::time::Instant)> = None;
        const DEDUP_WINDOW: Duration = Duration::from_secs(30);

        loop {
            match rx.recv().await {
                Ok(AgentEvent::DevicePending { device_id, ip, .. }) => {
                    if let Some((ref last_id, last_at)) = last_fired
                        && last_id == &device_id
                        && last_at.elapsed() < DEDUP_WINDOW
                    {
                        continue;
                    }
                    last_fired = Some((device_id.clone(), std::time::Instant::now()));

                    let short = device_id.chars().take(10).collect::<String>();
                    let body = format!("Device {short}… from {ip} is waiting for approval");
                    tokio::task::spawn_blocking(move || {
                        let _ = notify_rust::Notification::new()
                            .appname(APP_NAME)
                            .summary("OxiRemote — device pending")
                            .body(&body)
                            .timeout(notify_rust::Timeout::Milliseconds(8_000))
                            .show();
                    });
                }

                Ok(AgentEvent::TunnelDown { .. }) => {
                    const TUNNEL_DEDUP_KEY: &str = "tunnel_down";
                    if let Some((ref last_id, last_at)) = last_fired
                        && last_id == TUNNEL_DEDUP_KEY
                        && last_at.elapsed() < DEDUP_WINDOW
                    {
                        continue;
                    }
                    last_fired = Some((TUNNEL_DEDUP_KEY.to_string(), std::time::Instant::now()));

                    tokio::task::spawn_blocking(move || {
                        let _ = notify_rust::Notification::new()
                            .appname(APP_NAME)
                            .summary("OxiRemote — tunnel down")
                            .body("Tunnel went down — connections will fail.")
                            .timeout(notify_rust::Timeout::Milliseconds(8_000))
                            .show();
                    });
                }

                Ok(AgentEvent::SessionAgentEnded {
                    session_id,
                    agent_name,
                    duration_ms,
                    exit_code,
                }) => {
                    // Load toggle + subscriptions from SQLite on a blocking thread.
                    let db = db_path.clone();
                    let hid = host_id.clone();
                    let sid = session_id.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        load_agent_push_targets(&db, &hid, &sid)
                    })
                    .await;

                    match result {
                        Ok(Ok(Some(subs))) if !subs.is_empty() => {
                            send_agent_finished_push(
                                &http_client,
                                &vapid_keys,
                                subs,
                                &notify_subject,
                                AgentFinishedPushParams {
                                    host_id: &host_id,
                                    session_id: &session_id,
                                    agent_name: &agent_name,
                                    duration_ms,
                                    exit_code,
                                    // Tail lines: the ring buffer lives in AppState which
                                    // notifier does not hold. For v1 we pass empty; the
                                    // duration + exit code already provide useful context.
                                    tail_lines: Vec::new(),
                                },
                            )
                            .await;
                        }
                        Ok(Ok(Some(_))) => {
                            // Toggle enabled but no subscriptions registered yet — no-op.
                        }
                        Ok(Ok(None)) => {
                            // Toggle off — no push needed.
                        }
                        Ok(Err(err)) => {
                            warn!(error=%err, session=%session_id, "agent-end push DB lookup failed");
                        }
                        Err(err) => {
                            warn!(error=%err, session=%session_id, "agent-end push spawn_blocking failed");
                        }
                    }
                }

                Ok(_) => {}
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "notifier lagged event bus");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}
