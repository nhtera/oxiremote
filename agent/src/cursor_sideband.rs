//! WebRTC sideband transport for the host OS cursor.
//!
//! Polls the live cursor at ~60 Hz and forwards pose + shape updates over
//! a dedicated `cursor` DataChannel (negotiated stream id 3). The client
//! renders the sprite at network RTT instead of waiting for the next
//! video frame — eliminating the 33-100 ms cursor lag the in-frame
//! compositor produced.
//!
//! Wire protocol (JSON over the DC's `send_text`):
//! - Pose: `{"t":"p","x":<f32 0..1>,"y":<f32 0..1>,"id":<u64>,"h":<bool>}`
//! - Shape: `{"t":"s","id":<u64>,"w":<u32>,"h":<u32>,"hx":<u32>,"hy":<u32>,"px":"<base64-rgba>"}`
//!
//! Shape is only sent the first time the server sees a given HCURSOR id
//! (cached by id on the client). Pose is sent at most once per change.
//!
//! Windows-only for now — macOS keeps the SCK-composited in-frame cursor.

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use base64::Engine;
    use base64::engine::general_purpose;
    use desktop::cursor_windows;
    use tokio::sync::mpsc;
    use tracing::{debug, warn};
    use webrtc::data_channel::RTCDataChannel;
    use webrtc::data_channel::data_channel_state::RTCDataChannelState;

    /// Poll cadence. 60 Hz gives a smooth cursor without burning CPU —
    /// `GetCursorInfo` is a few µs.
    const POLL_INTERVAL_MS: u64 = 16;

    #[derive(Clone, Copy)]
    struct Snapshot {
        x: i32,
        y: i32,
        id: u64,
        hidden: bool,
    }

    /// Spawn the poll + send task. Lives for as long as the DataChannel
    /// is open; exits cleanly on `Closed` / `Closing`. Caller spawns this
    /// from the `on_open` callback so it doesn't run before negotiation
    /// completes.
    pub fn spawn(
        dc: Arc<RTCDataChannel>,
        monitor_origin: (i32, i32),
        monitor_w: u32,
        monitor_h: u32,
    ) {
        let (tx, rx) = mpsc::channel::<Snapshot>(8);
        let poll_dc = Arc::clone(&dc);
        // Blocking poller thread. Skips redundant emits so an idle cursor
        // doesn't burn the channel.
        std::thread::spawn(move || poll_loop(tx, poll_dc));
        tokio::spawn(forward_loop(rx, dc, monitor_origin, monitor_w, monitor_h));
    }

    fn poll_loop(tx: mpsc::Sender<Snapshot>, dc: Arc<RTCDataChannel>) {
        let mut last: Option<Snapshot> = None;
        loop {
            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            let state = match dc.ready_state() {
                RTCDataChannelState::Open => match cursor_windows::poll_state() {
                    Some(s) => s,
                    None => continue,
                },
                _ => break,
            };
            let snap = Snapshot {
                x: state.x,
                y: state.y,
                id: state.id,
                hidden: state.hidden,
            };
            if last.is_some_and(|l| same(&l, &snap)) {
                continue;
            }
            last = Some(snap);
            // Drop-newest backpressure — pose is purely positional, so a
            // stale frame doesn't matter; the next emit will catch up.
            if tx.try_send(snap).is_err()
                && tx.blocking_send(snap).is_err()
            {
                break;
            }
        }
        debug!("cursor sideband poller exited");
    }

    fn same(a: &Snapshot, b: &Snapshot) -> bool {
        a.x == b.x && a.y == b.y && a.id == b.id && a.hidden == b.hidden
    }

    async fn forward_loop(
        mut rx: mpsc::Receiver<Snapshot>,
        dc: Arc<RTCDataChannel>,
        origin: (i32, i32),
        w: u32,
        h: u32,
    ) {
        let mut sprites_sent: HashSet<u64> = HashSet::new();
        let w_f = w.max(1) as f32;
        let h_f = h.max(1) as f32;

        while let Some(snap) = rx.recv().await {
            if dc.ready_state() != RTCDataChannelState::Open {
                break;
            }
            // Ship the shape exactly once per new id, before its first pose.
            if !snap.hidden && !sprites_sent.contains(&snap.id) {
                let id = snap.id;
                let sprite = tokio::task::spawn_blocking(move || cursor_windows::fetch_sprite(id))
                    .await
                    .ok()
                    .flatten();
                if let Some(sp) = sprite {
                    let payload = serde_json::json!({
                        "t": "s",
                        "id": snap.id,
                        "w": sp.width,
                        "h": sp.height,
                        "hx": sp.hotspot_x,
                        "hy": sp.hotspot_y,
                        "px": general_purpose::STANDARD.encode(&sp.rgba),
                    });
                    if let Err(e) = dc.send_text(payload.to_string()).await {
                        warn!(error = %e, "cursor sideband: shape send failed");
                        continue;
                    }
                    sprites_sent.insert(snap.id);
                }
            }

            let nx = ((snap.x - origin.0) as f32 / w_f).clamp(0.0, 1.0);
            let ny = ((snap.y - origin.1) as f32 / h_f).clamp(0.0, 1.0);
            let pose = serde_json::json!({
                "t": "p",
                "x": nx,
                "y": ny,
                "id": snap.id,
                "h": snap.hidden,
            });
            if let Err(e) = dc.send_text(pose.to_string()).await {
                warn!(error = %e, "cursor sideband: pose send failed");
                break;
            }
        }
        debug!("cursor sideband forwarder exited");
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::spawn;

/// No-op on non-Windows targets. macOS keeps the SCK-composited cursor
/// in the captured frame; Linux desktop is deferred.
#[cfg(not(target_os = "windows"))]
pub fn spawn(
    _dc: std::sync::Arc<webrtc::data_channel::RTCDataChannel>,
    _monitor_origin: (i32, i32),
    _monitor_w: u32,
    _monitor_h: u32,
) {
}
