/// Screen capture abstraction and FPS-capped capture loop.
use std::time::{Duration, Instant};

use anyhow::Context;
use image::RgbaImage;
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tokio::sync::oneshot;
use tracing::{info, warn};
use xcap::Monitor;

use crate::encode::{FrameOutput, QualityTier, TileDiff, TileEncoder};

/// Wraps a single monitor handle for frame-by-frame capture.
pub struct ScreenCapture {
    monitor: Monitor,
    scale_factor: f32,
}

impl ScreenCapture {
    /// Open the primary (index 0) monitor.
    pub fn primary() -> anyhow::Result<Self> {
        let monitors = Monitor::all().context("list monitors")?;
        let monitor = monitors.into_iter().next().context("no monitors found")?;
        let scale_factor = monitor.scale_factor().unwrap_or(1.0).max(1.0);
        Ok(ScreenCapture { monitor, scale_factor })
    }

    /// Capture a single full-resolution frame from the monitor.
    pub fn next_frame(&self) -> anyhow::Result<RgbaImage> {
        self.monitor.capture_image().context("capture_image")
    }

    /// Width of this monitor in logical pixels.
    pub fn width(&self) -> anyhow::Result<u32> {
        self.monitor.width().context("monitor width")
    }

    /// Height of this monitor in logical pixels.
    pub fn height(&self) -> anyhow::Result<u32> {
        self.monitor.height().context("monitor height")
    }

    /// Physical-to-logical pixel ratio from xcap (clamped to ≥ 1.0).
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }
}

/// Read the primary monitor's scale factor without keeping the capture handle.
/// Used by the session runner to pre-compute output dimensions before spawning
/// the capture loop. Returns 1.0 if the monitor cannot be opened.
pub fn primary_scale_factor() -> f32 {
    Monitor::all()
        .ok()
        .and_then(|ms| ms.into_iter().next())
        .and_then(|m| m.scale_factor().ok())
        .unwrap_or(1.0)
        .max(1.0)
}

/// Frame-rate constants per quality tier (milliseconds per frame).
const FPS_HIGH_MS: u64 = 33; // ~30 FPS
const FPS_MED_MS: u64 = 66; // ~15 FPS
const FPS_LOW_MS: u64 = 125; // ~8 FPS

fn frame_interval(tier: QualityTier) -> Duration {
    match tier {
        QualityTier::High => Duration::from_millis(FPS_HIGH_MS),
        QualityTier::Med => Duration::from_millis(FPS_MED_MS),
        QualityTier::Low => Duration::from_millis(FPS_LOW_MS),
    }
}

/// Runs the capture → diff → encode pipeline at the target FPS.
///
/// Designed to be spawned via `tokio::task::spawn_blocking` because
/// `Monitor::capture_image()` is synchronous. The loop terminates when
/// the receiver of `tx` is dropped.
pub struct CaptureLoop;

impl CaptureLoop {
    /// Blocking capture loop. Call from `spawn_blocking`.
    ///
    /// - `tier`: quality tier controlling FPS + JPEG quality + tier resize.
    /// - `tx`: frame sink; channel **full** drops newest (never blocks the
    ///   capture thread); channel **closed** exits the loop cleanly.
    /// - `scale_factor`: xcap physical/logical ratio; `1.0` is safe on non-HiDPI.
    /// - `force_iframe_rx`: optional oneshot that, when fired, resets the tile
    ///   diff so the next emitted frame contains every tile (equivalent to an
    ///   H.264 IDR for a joining viewer).
    ///
    /// **Precondition:** Caller must verify `desktop::desktop_available()`
    /// is `true` before spawning this loop. We do not re-probe here — that
    /// would duplicate the TCC prompt on macOS first-run.
    pub fn run(
        tier: QualityTier,
        tx: Sender<FrameOutput>,
        scale_factor: f32,
        mut force_iframe_rx: Option<oneshot::Receiver<()>>,
    ) {
        let capture = match ScreenCapture::primary() {
            Ok(c) => c,
            Err(err) => {
                warn!(error = %err, "CaptureLoop: failed to open primary monitor");
                return;
            }
        };

        info!(
            tier = ?tier,
            scale_factor,
            "CaptureLoop started"
        );

        let interval = frame_interval(tier);
        let mut diff = TileDiff::new();
        let mut frame_count: u64 = 0;
        let mut dropped_count: u64 = 0;

        loop {
            let frame_start = Instant::now();

            // I-frame request arrived? Reset the diff so the next emitted
            // frame contains every tile. Cheap non-blocking poll.
            if let Some(mut rx) = force_iframe_rx.take() {
                match rx.try_recv() {
                    Ok(()) => {
                        diff.reset();
                        info!("CaptureLoop: force-iframe received, next frame is full");
                    }
                    Err(oneshot::error::TryRecvError::Empty) => {
                        // Not yet — put it back for next iteration.
                        force_iframe_rx = Some(rx);
                    }
                    Err(oneshot::error::TryRecvError::Closed) => {
                        // Sender dropped without firing — stop polling.
                    }
                }
            }

            // Capture raw frame.
            let raw = match capture.next_frame() {
                Ok(img) => img,
                Err(err) => {
                    warn!(error = %err, "CaptureLoop: capture error, continuing");
                    std::thread::sleep(interval);
                    continue;
                }
            };

            // Resize + diff + encode.
            let output = TileEncoder::process_frame(raw, tier, scale_factor, &mut diff);

            // Only emit non-idle frames. Drop-newest on backpressure so the
            // capture thread never stalls behind a slow consumer.
            if !output.tiles.is_empty() {
                match tx.try_send(output) {
                    Ok(()) => frame_count += 1,
                    Err(TrySendError::Full(_)) => {
                        dropped_count += 1;
                        tracing::debug!(
                            dropped = dropped_count,
                            "CaptureLoop: backpressure drop"
                        );
                    }
                    Err(TrySendError::Closed(_)) => {
                        info!("CaptureLoop: channel closed, stopping");
                        break;
                    }
                }
            }

            // Sleep the remaining budget for this frame interval.
            let elapsed = frame_start.elapsed();
            if let Some(remaining) = interval.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }

        info!(
            frames_sent = frame_count,
            frames_dropped = dropped_count,
            "CaptureLoop stopped"
        );
    }
}
