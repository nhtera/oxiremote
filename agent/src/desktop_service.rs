/// Desktop session registry — owns active `DesktopSession` handles.
///
/// One `DesktopService` lives in `AppState`. It tracks at most one session per
/// device ID (single-viewer v1). On a second connection to the same device the
/// old session is evicted (its channels close, capture loop stops).
use dashmap::DashMap;

/// A lightweight token that represents one active desktop WS session.
/// The actual heavy state (RTCPeerConnection, capture task) lives inside
/// the session task; this registry only holds the close signal plus
/// optional ABR observation publisher (phase-03; Some only when an
/// h264/vp9/av1 pipeline is the active video transport).
pub struct SessionHandle {
    pub close_tx: tokio::sync::oneshot::Sender<()>,
    /// Phase-03: snapshot subscription point for the stats SSE endpoint.
    /// Populated by `attach_abr_tx` once a webrtc video pipeline has wired
    /// its broadcast. JPEG sessions leave this `None`.
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    pub abr_tx: Option<tokio::sync::broadcast::Sender<crate::desktop_abr::AbrObservation>>,
    /// Codec label paired with `abr_tx` — surfaced by the stats SSE so the
    /// overlay shows the actual negotiated codec ("h264" | "vp9" | "av1")
    /// instead of a hardcoded value.
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    pub pipeline_label: Option<&'static str>,
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
                #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
                abr_tx: None,
                #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
                pipeline_label: None,
            },
        );
        rx
    }

    /// Attach the active webrtc pipeline's ABR observation publisher +
    /// codec label to the registered session. Called from each pipeline
    /// branch in `desktop_ws.rs` after the encoder is spawned. No-op if the
    /// session was evicted between `register` and this call (race tolerated
    /// — observations just aren't routable until the next register).
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    pub fn attach_abr_tx(
        &self,
        device_id: &str,
        tx: tokio::sync::broadcast::Sender<crate::desktop_abr::AbrObservation>,
        pipeline_label: &'static str,
    ) {
        if let Some(mut entry) = self.sessions.get_mut(device_id) {
            entry.abr_tx = Some(tx);
            entry.pipeline_label = Some(pipeline_label);
        }
    }

    /// Subscribe a new ABR observation receiver + the codec label for
    /// `device_id`. Returns `None` when no webrtc session is currently
    /// registered (caller should 404). Each call yields an independent
    /// receiver — the broadcast fan-out lets controller + stats SSE attach
    /// concurrently.
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    pub fn subscribe_abr(
        &self,
        device_id: &str,
    ) -> Option<(
        tokio::sync::broadcast::Receiver<crate::desktop_abr::AbrObservation>,
        &'static str,
    )> {
        self.sessions.get(device_id).and_then(|entry| {
            let tx = entry.abr_tx.as_ref()?;
            let label = entry.pipeline_label.unwrap_or("webrtc");
            Some((tx.subscribe(), label))
        })
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
