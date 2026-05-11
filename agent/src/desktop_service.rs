/// Desktop session registry — owns active `DesktopSession` handles.
///
/// One `DesktopService` lives in `AppState`. It tracks at most one session per
/// device ID (single-viewer v1). On a second connection to the same device the
/// old session is evicted (its channels close, capture loop stops).
use dashmap::DashMap;

/// A lightweight token that represents one active desktop WS session.
/// The actual heavy state (RTCPeerConnection, capture task) lives inside
/// the session task; this registry only holds the close signal plus
/// optional ABR observation publisher (phase-03; Some only when the
/// h264 pipeline is the active video transport).
pub struct SessionHandle {
    pub close_tx: tokio::sync::oneshot::Sender<()>,
    /// Phase-03: snapshot subscription point for the stats SSE endpoint.
    /// Populated by `attach_abr_tx` once the h264 pipeline has wired its
    /// own broadcast. JPEG sessions leave this `None` (no per-session
    /// observations until phase-03 extends to the JPEG path).
    #[cfg(feature = "h264")]
    pub abr_tx: Option<tokio::sync::broadcast::Sender<crate::desktop_abr::AbrObservation>>,
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

        self.sessions.insert(
            device_id.to_string(),
            SessionHandle {
                close_tx: tx,
                #[cfg(feature = "h264")]
                abr_tx: None,
            },
        );
        rx
    }

    /// Attach the h264 pipeline's ABR observation publisher to the active
    /// session for `device_id`. Called from `run_h264_session` after the
    /// pipeline is spawned. No-op if the session has been evicted in the
    /// gap between `register` and this call (race tolerated — observations
    /// just aren't routable until the next register).
    #[cfg(feature = "h264")]
    pub fn attach_abr_tx(
        &self,
        device_id: &str,
        tx: tokio::sync::broadcast::Sender<crate::desktop_abr::AbrObservation>,
    ) {
        if let Some(mut entry) = self.sessions.get_mut(device_id) {
            entry.abr_tx = Some(tx);
        }
    }

    /// Subscribe a new ABR observation receiver for `device_id`. Returns
    /// `None` when no h264 session is currently registered (caller should
    /// 404). Each call yields an independent receiver — the broadcast
    /// fan-out lets controller + stats SSE attach concurrently.
    #[cfg(feature = "h264")]
    pub fn subscribe_abr(
        &self,
        device_id: &str,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::desktop_abr::AbrObservation>> {
        self.sessions
            .get(device_id)
            .and_then(|entry| entry.abr_tx.as_ref().map(|tx| tx.subscribe()))
    }

    /// Remove a session (called on WS close).
    pub fn remove(&self, device_id: &str) {
        self.sessions.remove(device_id);
    }

    /// Forcibly evict the session for `device_id`, closing the WebSocket and
    /// stopping the capture loop. Returns true when a session was kicked.
    /// Used by the operator-side "Disconnect" action which kicks an active
    /// device without revoking its trust (so it can reconnect later).
    pub fn kick(&self, device_id: &str) -> bool {
        if let Some((_, h)) = self.sessions.remove(device_id) {
            let _ = h.close_tx.send(());
            true
        } else {
            false
        }
    }

    /// Returns true when there is an active desktop session for `device_id`.
    /// Used by `active_sessions_snapshot` to classify a device as "desktop".
    pub fn has_session(&self, device_id: &str) -> bool {
        self.sessions.contains_key(device_id)
    }

    /// Return a snapshot of all currently-active device IDs.
    /// Used by `active_sessions_snapshot` to seed the device-id set.
    pub fn active_device_ids(&self) -> Vec<String> {
        self.sessions.iter().map(|e| e.key().clone()).collect()
    }
}
