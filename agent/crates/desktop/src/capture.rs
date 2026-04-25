/// Screen capture abstraction and FPS-capped capture loop.
use std::time::{Duration, Instant};

use anyhow::Context;
use image::RgbaImage;
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tokio::sync::oneshot;
use tracing::{info, warn};
use xcap::Monitor;

use crate::encode::{quality_resize, FrameOutput, QualityTier, TileDiff, TileEncoder};

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

    /// Raw-BGRA variant for the Phase 03 H.264 pipeline. Shares the capture
    /// clock + force-iframe signal with `run`, but skips tile diff + JPEG
    /// encode and instead emits resized BGRA bytes for the `H264Encoder` to
    /// consume. The H.264 encoder handles its own keyframe logic, so we do
    /// not clear any diff state here — we just forward `force_iframe` as a
    /// flag on the next emitted frame.
    ///
    /// `resolution_tier` is captured ONCE and controls the output dimensions
    /// for the lifetime of the loop — H.264 encoders are built at a fixed
    /// resolution and cannot be resized mid-stream. `fps_rx` is polled every
    /// iteration so the loop's frame cadence tracks the user's tier slider
    /// in real time without restarting the capture (Low ≈ 8 fps,
    /// Med ≈ 15 fps, High ≈ 30 fps).
    pub fn run_bgra(
        resolution_tier: QualityTier,
        fps_rx: tokio::sync::watch::Receiver<QualityTier>,
        tx: Sender<RawBgraFrame>,
        scale_factor: f32,
        force_iframe_rx: Option<oneshot::Receiver<()>>,
    ) {
        // ── Pick a capture backend ────────────────────────────────────────
        // Prefer ScreenCaptureKit on macOS 12.3+: GPU-resized BGRA arrives
        // pre-formatted, so the encoder gets frames with zero CPU resize and
        // zero RGBA→BGRA conversion. Fall back to xcap (CGWindowListCreateImage)
        // anywhere SCK init fails — older macOS, missing Screen Recording
        // permission, Linux, Windows.
        #[cfg(target_os = "macos")]
        if let Some((screen_w, screen_h)) = primary_dims() {
            // xcap's `Monitor::width()` returns logical points (CGDisplayBounds),
            // matching what `desktop_ws_inner::primary_screen_dimensions()` and
            // the encoder build use. So feed it straight into resize_dims.
            let (target_w, target_h) =
                crate::encode::resize_dims(screen_w, screen_h, resolution_tier);
            // SCK target FPS is the ceiling; we still gate on fps_rx in the
            // hot loop. Pass the highest tier's FPS so SCK never throttles
            // below what the user might select mid-session.
            let target_fps = max_tier_fps_hz();
            match crate::sck::SckCapture::new(target_w, target_h, target_fps) {
                Ok(sck) => {
                    info!(
                        resolution_tier = ?resolution_tier,
                        initial_fps_tier = ?*fps_rx.borrow(),
                        target_w, target_h, target_fps,
                        "CaptureLoop::run_bgra started (ScreenCaptureKit)"
                    );
                    return run_bgra_sck(sck, fps_rx, tx, force_iframe_rx);
                }
                Err(err) => warn!(error = %err, "ScreenCaptureKit init failed, falling back to xcap"),
            }
        }

        let capture = match ScreenCapture::primary() {
            Ok(c) => c,
            Err(err) => {
                warn!(error = %err, "CaptureLoop::run_bgra: failed to open primary monitor");
                return;
            }
        };
        info!(
            resolution_tier = ?resolution_tier,
            initial_fps_tier = ?*fps_rx.borrow(),
            scale_factor,
            "CaptureLoop::run_bgra started (xcap)"
        );

        run_bgra_xcap(capture, resolution_tier, fps_rx, tx, scale_factor, force_iframe_rx);
    }
}

// ── Backend-specific drivers ─────────────────────────────────────────────────

/// xcap-backed driver. Captures full physical RGBA, runs `quality_resize` +
/// `rgba_to_bgra`, then forwards a `RawBgraFrame`.
fn run_bgra_xcap(
    capture: ScreenCapture,
    resolution_tier: QualityTier,
    fps_rx: tokio::sync::watch::Receiver<QualityTier>,
    tx: Sender<RawBgraFrame>,
    scale_factor: f32,
    mut force_iframe_rx: Option<oneshot::Receiver<()>>,
) {
    let mut frame_count: u64 = 0;
    let mut dropped_count: u64 = 0;

    loop {
        let frame_start = Instant::now();
        let interval = frame_interval(*fps_rx.borrow());

        let mut force_idr = false;
        if let Some(mut rx) = force_iframe_rx.take() {
            match rx.try_recv() {
                Ok(()) => {
                    force_idr = true;
                    info!("CaptureLoop::run_bgra: force-iframe received");
                }
                Err(oneshot::error::TryRecvError::Empty) => force_iframe_rx = Some(rx),
                Err(oneshot::error::TryRecvError::Closed) => {}
            }
        }

        let raw = match capture.next_frame() {
            Ok(img) => img,
            Err(err) => {
                warn!(error = %err, "CaptureLoop::run_bgra: capture error");
                std::thread::sleep(interval);
                continue;
            }
        };

        let resized = quality_resize(raw, resolution_tier, scale_factor);
        let (width, height) = (resized.width(), resized.height());
        let bytes = rgba_to_bgra(resized.as_raw());

        match tx.try_send(RawBgraFrame {
            bytes,
            width,
            height,
            force_idr,
        }) {
            Ok(()) => frame_count += 1,
            Err(TrySendError::Full(_)) => {
                dropped_count += 1;
                tracing::debug!(dropped = dropped_count, "run_bgra: backpressure drop");
            }
            Err(TrySendError::Closed(_)) => {
                info!("CaptureLoop::run_bgra: channel closed, stopping");
                break;
            }
        }

        let elapsed = frame_start.elapsed();
        if let Some(remaining) = interval.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }

    info!(
        frames_sent = frame_count,
        frames_dropped = dropped_count,
        "CaptureLoop::run_bgra (xcap) stopped"
    );
}

/// SCK-backed driver. SCK already delivers BGRA at the configured stream
/// dimensions via the GPU IOSurface — no resize, no convert. We just gate on
/// `fps_rx` (drop frames the user's tier doesn't want), apply force-IDR, and
/// forward.
#[cfg(target_os = "macos")]
fn run_bgra_sck(
    mut sck: crate::sck::SckCapture,
    fps_rx: tokio::sync::watch::Receiver<QualityTier>,
    tx: Sender<RawBgraFrame>,
    mut force_iframe_rx: Option<oneshot::Receiver<()>>,
) {
    let mut frame_count: u64 = 0;
    let mut dropped_count: u64 = 0;
    // Emit-throttle so we honour the user's tier. SCK runs at its max
    // configured cadence; we drop frames that arrive sooner than the
    // currently-selected tier interval.
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    loop {
        let frame = match sck.next_frame_blocking() {
            Some(f) => f,
            None => {
                info!("CaptureLoop::run_bgra: SCK stream ended");
                break;
            }
        };

        let interval = frame_interval(*fps_rx.borrow());

        let mut force_idr = false;
        if let Some(mut rx) = force_iframe_rx.take() {
            match rx.try_recv() {
                Ok(()) => {
                    force_idr = true;
                    info!("CaptureLoop::run_bgra: force-iframe received");
                }
                Err(oneshot::error::TryRecvError::Empty) => force_iframe_rx = Some(rx),
                Err(oneshot::error::TryRecvError::Closed) => {}
            }
        }

        // Tier gate: drop frames arriving faster than the user's current tier.
        // force_idr always emits to keep PLI recovery snappy.
        let now = Instant::now();
        if !force_idr && now.duration_since(last_emit) < interval {
            continue;
        }
        last_emit = now;

        match tx.try_send(RawBgraFrame {
            bytes: frame.bytes,
            width: frame.width,
            height: frame.height,
            force_idr,
        }) {
            Ok(()) => frame_count += 1,
            Err(TrySendError::Full(_)) => {
                dropped_count += 1;
                tracing::debug!(dropped = dropped_count, "run_bgra: backpressure drop");
            }
            Err(TrySendError::Closed(_)) => {
                info!("CaptureLoop::run_bgra: channel closed, stopping");
                break;
            }
        }
    }

    info!(
        frames_sent = frame_count,
        frames_dropped = dropped_count,
        "CaptureLoop::run_bgra (sck) stopped"
    );
}

/// Look up the primary monitor's physical pixel dimensions. Used to decide
/// SCK output dims — see `run_bgra`.
#[cfg(target_os = "macos")]
fn primary_dims() -> Option<(u32, u32)> {
    let cap = ScreenCapture::primary().ok()?;
    Some((cap.width().ok()?, cap.height().ok()?))
}

/// Highest configured frame interval expressed in Hz. Used so SCK never
/// throttles below what the user's tier slider can request.
#[cfg(target_os = "macos")]
fn max_tier_fps_hz() -> u32 {
    let high_ms = frame_interval(QualityTier::High).as_millis().max(1) as u32;
    1000 / high_ms
}


/// One captured frame already resized to the logical/tier dimensions, ready
/// for H.264 encode. Byte order is BGRA (matches `kCVPixelFormatType_32BGRA`
/// and the `yuvutils-rs` BGRA→I420 path used by OpenH264).
#[derive(Debug)]
pub struct RawBgraFrame {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Set when the pipeline must mark the next encoded frame as an IDR.
    /// Driven by client PLI over RTCP or by session-open cold-start.
    pub force_idr: bool,
}

/// Convert an RGBA8 byte slice in-place-style to a BGRA8 Vec. xcap delivers
/// RGBA; both our H.264 encoder backends (VT's `kCVPixelFormatType_32BGRA`
/// and OpenH264's `yuvutils-rs::bgra_to_yuv420`) take BGRA.
///
/// Per-pixel swap of R↔B. Could be SIMD-accelerated later; at typical tier
/// resolutions (≤ 1512×982 logical) this is ~1.5 M pixels ≈ < 3 ms on an
/// M-series core — well under the 33 ms frame budget.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    debug_assert!(rgba.len() % 4 == 0);
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        out.push(p[2]); // B
        out.push(p[1]); // G
        out.push(p[0]); // R
        out.push(p[3]); // A
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_bgra_swaps_r_and_b() {
        let rgba = [
            0x11, 0x22, 0x33, 0xFF, // R, G, B, A
            0xAA, 0xBB, 0xCC, 0x80,
        ];
        let bgra = rgba_to_bgra(&rgba);
        assert_eq!(
            &bgra[..],
            &[0x33, 0x22, 0x11, 0xFF, 0xCC, 0xBB, 0xAA, 0x80]
        );
    }

    #[test]
    fn rgba_to_bgra_empty_input() {
        assert!(rgba_to_bgra(&[]).is_empty());
    }
}
