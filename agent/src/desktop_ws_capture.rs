/// Capture task and tile-send sink for the remote desktop WebRTC/WS transport.
///
/// Responsibilities:
/// - Spawn `CaptureLoop::run` on a blocking thread.
/// - Drain `FrameOutput` from the channel and encode each tile into the
///   5-byte-header binary frame format.
/// - Route encoded frames to either an `RTCDataChannel` or a WS binary sink.
/// - Support quality changes by dropping the current capture task and spawning
///   a fresh one on a `watch` channel notification.
#[cfg(feature = "desktop")]
pub mod inner {
    use std::sync::Arc;

    use bytes::{BufMut, Bytes, BytesMut};
    use desktop::capture::CaptureLoop;
    use desktop::{FrameOutput, QualityTier};
    use tokio::sync::{mpsc, watch};
    use tracing::{info, warn};
    use webrtc::data_channel::RTCDataChannel;

    /// Where to send encoded tile frames.
    pub enum Sink {
        /// Send over an open WebRTC DataChannel.
        DataChannel(Arc<RTCDataChannel>),
        /// Send as binary WS messages (fallback path).
        WsBinary(mpsc::Sender<Bytes>),
    }

    /// Build the 5-byte-header binary frame for one tile.
    ///
    /// Format: `[x_hi, x_lo, y_hi, y_lo, flags, ...jpeg]`
    /// `flags & 1 == 1` only on the last tile of a `FrameOutput`.
    pub fn encode_tile_frame(x: u16, y: u16, flags: u8, jpeg: Bytes) -> Bytes {
        let mut buf = BytesMut::with_capacity(5 + jpeg.len());
        buf.put_u16(x);
        buf.put_u16(y);
        buf.put_u8(flags);
        buf.extend_from_slice(&jpeg);
        buf.freeze()
    }

    /// Spawn the capture → drain → send pipeline.
    ///
    /// Listens on `quality_rx` for quality-change notifications. On change,
    /// the current `CaptureLoop` blocking task is killed (by dropping its
    /// channel receiver) and a fresh one is spawned at the new tier.
    ///
    /// The whole capture pipeline stops when `shutdown_rx` fires or when the
    /// `Sink` send fails (WS/DC closed).
    pub fn spawn_capture_pipeline(
        initial_tier: QualityTier,
        sink: Sink,
        mut quality_rx: watch::Receiver<QualityTier>,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        tokio::spawn(async move {
            let sink = Arc::new(sink);
            let mut current_tier = initial_tier;

            loop {
                // Channel pair: blocking CaptureLoop → async drain task.
                let (frame_tx, mut frame_rx) = mpsc::channel::<FrameOutput>(8);
                let tier = current_tier;

                // Spawn the blocking capture loop.
                let _capture_handle =
                    tokio::task::spawn_blocking(move || CaptureLoop::run(tier, frame_tx));

                info!(tier = ?tier, "capture pipeline started");

                // Drain frames until quality changes or shutdown fires.
                loop {
                    tokio::select! {
                        // Prefer shutdown over quality change for clean exit.
                        biased;

                        _ = &mut shutdown_rx => {
                            info!("capture pipeline: shutdown received");
                            return;
                        }

                        _ = quality_rx.changed() => {
                            let new_tier = *quality_rx.borrow();
                            if new_tier != current_tier {
                                info!(old = ?current_tier, new = ?new_tier, "quality change → restart capture");
                                current_tier = new_tier;
                                // Drop frame_rx — this signals CaptureLoop to stop.
                                drop(frame_rx);
                                break; // Restart outer loop with new tier.
                            }
                        }

                        frame = frame_rx.recv() => {
                            let Some(frame_output) = frame else {
                                // CaptureLoop exited (e.g. monitor disappeared).
                                warn!("CaptureLoop channel closed unexpectedly");
                                return;
                            };

                            if frame_output.tiles.is_empty() {
                                continue; // Idle frame — skip.
                            }

                            let last_idx = frame_output.tiles.len() - 1;
                            let sink_ref = Arc::clone(&sink);

                            for (i, tile) in frame_output.tiles.into_iter().enumerate() {
                                let flags: u8 = if i == last_idx { 1 } else { 0 };
                                let frame_bytes =
                                    encode_tile_frame(tile.x, tile.y, flags, tile.jpeg);

                                let send_ok = match sink_ref.as_ref() {
                                    Sink::DataChannel(dc) => {
                                        dc.send(&frame_bytes).await.is_ok()
                                    }
                                    Sink::WsBinary(tx) => tx.send(frame_bytes).await.is_ok(),
                                };

                                if !send_ok {
                                    info!("capture pipeline: sink closed, stopping");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(feature = "desktop")]
pub use inner::{Sink, spawn_capture_pipeline};
