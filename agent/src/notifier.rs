// Desktop notifications via `notify-rust`. Independent of the tray runtime
// (which is `#![allow(dead_code)]` until Phase 06 main-thread arbitration is
// solved). Silently no-ops on hosts without a notification daemon (e.g.
// headless servers, Codespaces).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::events::{AgentEvent, EventBus};

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

/// Subscribe to the event bus and surface device-pending and tunnel-down events
/// as desktop notifications. Throttles per-key (device or "tunnel") so
/// a flapping client/process cannot spam.
pub fn spawn_event_notifier(bus: Arc<EventBus>) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        // (dedup_key, last_fired_instant) — single-slot; one category at a time.
        let mut last_fired: Option<(String, std::time::Instant)> = None;
        const DEDUP_WINDOW: Duration = Duration::from_secs(30);

        loop {
            match rx.recv().await {
                Ok(AgentEvent::DevicePending { device_id, ip, .. }) => {
                    if let Some((ref last_id, last_at)) = last_fired
                        && last_id == &device_id && last_at.elapsed() < DEDUP_WINDOW {
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
                        && last_id == TUNNEL_DEDUP_KEY && last_at.elapsed() < DEDUP_WINDOW {
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
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "notifier lagged event bus");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}
