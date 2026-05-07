// Bounded in-memory ring buffer of recent shell-lifecycle events.
//
// Phase 06c / H11: surfaces the last 50 `ShellOpened`/`ShellClosed` events to
// the host dashboard via `GET /api/agent/sessions/recent`. Without this pull
// endpoint, a dashboard tab opened *after* a shell already spawned would never
// see the event — the broadcast bus has no replay semantics. The dashboard
// polls this on mount, then listens to the SSE stream for live updates.
//
// Capacity is 50 entries, fixed. Logs tail the same data via `tracing` so the
// audit trail isn't bound by the buffer — the buffer is purely a UX
// convenience for the dashboard view.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tracing::warn;

use crate::events::{AgentEvent, EventBus};

pub const RECENT_SESSIONS_CAP: usize = 50;

pub type RecentSessions = Arc<Mutex<VecDeque<AgentEvent>>>;

pub fn new_buffer() -> RecentSessions {
    Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_SESSIONS_CAP)))
}

/// Subscribe the buffer to the bus. Drops the oldest entry on overflow so the
/// buffer never grows past `RECENT_SESSIONS_CAP` even under sustained churn.
/// On `RecvError::Lagged` (slow consumer fell behind the broadcast capacity)
/// we simply skip ahead — the live tail of events still lands, and a missed
/// burst is acceptable for an audit-log convenience buffer.
pub fn spawn_subscriber(buffer: RecentSessions, bus: Arc<EventBus>) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if matches!(event, AgentEvent::ShellOpened { .. } | AgentEvent::ShellClosed { .. })
                    {
                        // Recover from poisoning — the buffer is plain data, no
                        // invariant a panicked holder could have violated.
                        let mut guard = buffer.lock().unwrap_or_else(|p| p.into_inner());
                        if guard.len() >= RECENT_SESSIONS_CAP {
                            guard.pop_front();
                        }
                        guard.push_back(event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "recent_sessions subscriber lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Snapshot of the buffer for the API handler. Cloned to avoid holding the
/// mutex across the response-building code path.
pub fn snapshot(buffer: &RecentSessions) -> Vec<AgentEvent> {
    buffer
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AgentEvent;

    fn opened(id: &str) -> AgentEvent {
        AgentEvent::ShellOpened {
            device_id: "dev-1".into(),
            session_id: id.into(),
            command: "/bin/zsh".into(),
            cwd: None,
            ts: 0,
        }
    }

    #[tokio::test]
    async fn subscriber_records_shell_events() {
        let bus = EventBus::new();
        let buf = new_buffer();
        spawn_subscriber(buf.clone(), bus.clone());

        bus.send(opened("a"));
        bus.send(opened("b"));

        // Yield to let the subscriber drain.
        for _ in 0..10 {
            tokio::task::yield_now().await;
            if snapshot(&buf).len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let snap = snapshot(&buf);
        assert_eq!(snap.len(), 2);
    }

    #[tokio::test]
    async fn subscriber_evicts_oldest_when_full() {
        let bus = EventBus::new();
        let buf = new_buffer();
        spawn_subscriber(buf.clone(), bus.clone());

        for i in 0..(RECENT_SESSIONS_CAP + 5) {
            bus.send(opened(&format!("s-{i}")));
        }
        // Drain.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if snapshot(&buf).len() == RECENT_SESSIONS_CAP {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let snap = snapshot(&buf);
        assert_eq!(snap.len(), RECENT_SESSIONS_CAP);
        // First 5 were evicted; oldest retained should be `s-5`.
        match &snap[0] {
            AgentEvent::ShellOpened { session_id, .. } => assert_eq!(session_id, "s-5"),
            _ => panic!("unexpected event variant"),
        }
    }

    #[tokio::test]
    async fn subscriber_ignores_unrelated_events() {
        let bus = EventBus::new();
        let buf = new_buffer();
        spawn_subscriber(buf.clone(), bus.clone());

        bus.send(AgentEvent::DeviceConnected { device_id: "dev-1".into() });
        bus.send(opened("a"));

        for _ in 0..10 {
            tokio::task::yield_now().await;
            if !snapshot(&buf).is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let snap = snapshot(&buf);
        assert_eq!(snap.len(), 1);
    }
}
