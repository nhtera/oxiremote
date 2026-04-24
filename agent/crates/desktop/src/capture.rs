/// Screen capture abstraction and FPS-capped capture loop.
use std::time::{Duration, Instant};

use anyhow::Context;
use image::RgbaImage;
use tokio::sync::mpsc::Sender;
use tracing::{info, warn};
use xcap::Monitor;

use crate::encode::{FrameOutput, QualityTier, TileDiff, TileEncoder};

/// Wraps a single monitor handle for frame-by-frame capture.
pub struct ScreenCapture {
    monitor: Monitor,
}

impl ScreenCapture {
    /// Open the primary (index 0) monitor.
    pub fn primary() -> anyhow::Result<Self> {
        let monitors = Monitor::all().context("list monitors")?;
        let monitor = monitors.into_iter().next().context("no monitors found")?;
        Ok(ScreenCapture { monitor })
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
    /// Captures frames at the FPS defined by `tier`, diffs tiles, encodes
    /// changed tiles to JPEG, and sends `FrameOutput` values on `tx`.
    /// Idle frames (zero changed tiles) are not emitted.
    ///
    /// **Precondition:** Caller must verify `desktop::desktop_available()`
    /// is `true` before spawning this loop. We do not re-probe here — that
    /// would duplicate the TCC prompt on macOS first-run.
    pub fn run(tier: QualityTier, tx: Sender<FrameOutput>) {
        let capture = match ScreenCapture::primary() {
            Ok(c) => c,
            Err(err) => {
                warn!(error = %err, "CaptureLoop: failed to open primary monitor");
                return;
            }
        };

        info!(tier = ?tier, "CaptureLoop started");

        let interval = frame_interval(tier);
        let mut diff = TileDiff::new();
        let mut frame_count: u64 = 0;

        loop {
            let frame_start = Instant::now();

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
            let output = TileEncoder::process_frame(raw, tier, &mut diff);

            // Only emit non-idle frames.
            if !output.tiles.is_empty() {
                if tx.blocking_send(output).is_err() {
                    // Receiver dropped — exit loop cleanly.
                    info!("CaptureLoop: channel closed, stopping");
                    break;
                }
                frame_count += 1;
            }

            // Sleep the remaining budget for this frame interval.
            let elapsed = frame_start.elapsed();
            if let Some(remaining) = interval.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }

        info!(frames_sent = frame_count, "CaptureLoop stopped");
    }
}
