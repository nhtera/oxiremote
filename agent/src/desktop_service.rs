/// Desktop session registry — owns active `DesktopSession` handles.
///
/// One `DesktopService` lives in `AppState`. It tracks at most one session per
/// device ID (single-viewer v1). On a second connection to the same device the
/// old session is evicted (its channels close, capture loop stops).
use dashmap::DashMap;

/// A lightweight token that represents one active desktop WS session.
/// The actual heavy state (RTCPeerConnection, capture task) lives inside
/// the session task; this registry only holds the close signal.
pub struct SessionHandle {
    pub close_tx: tokio::sync::oneshot::Sender<()>,
}

/// Central registry of active desktop sessions, keyed by device ID.
pub struct DesktopService {
    sessions: DashMap<String, SessionHandle>,
}

impl DesktopService {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Register a new session. If one already exists for `device_id`, close
    /// it first (single-viewer policy). Returns the close receiver that the
    /// new session should await.
    pub fn register(&self, device_id: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        if let Some((_, old)) = self.sessions.remove(device_id) {
            let _ = old.close_tx.send(());
        }

        self.sessions
            .insert(device_id.to_string(), SessionHandle { close_tx: tx });
        rx
    }

    /// Remove a session (called on WS close).
    pub fn remove(&self, device_id: &str) {
        self.sessions.remove(device_id);
    }
}
