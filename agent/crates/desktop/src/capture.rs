/// Screen capture abstraction and FPS-capped capture loop.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use image::RgbaImage;
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tokio::sync::{oneshot, Notify};
use tracing::{info, warn};
use xcap::Monitor;

use crate::encode::{
    quality_resize, FrameOutput, QualityTier, TileDiff, TileEncoder, IDLE_INTERVAL_MAX_MS,
};

/// Wake the capture loop on any input event. JPEG path uses this to break out
/// of an idle-backoff sleep so the next frame after a mouse jiggle ships at
/// active cadence. Constructed in the WS layer; cloned into both the input
/// dispatch path (notify_one) and the capture loop (await on notified()).
pub type InputWake = Arc<Notify>;

/// Wraps a single monitor handle for frame-by-frame capture.
pub struct ScreenCapture {
    monitor: Monitor,
    scale_factor: f32,
    /// Monitor's top-left in virtual-screen coords, cached at open time.
    /// Windows cursor overlay uses this to map `GetCursorInfo`'s screen-space
    /// `ptScreenPos` into image-local coords. Always (0, 0) on macOS/Linux.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    origin: (i32, i32),
}

impl ScreenCapture {
    /// Open the primary (index 0) monitor.
    pub fn primary() -> anyhow::Result<Self> {
        let monitors = Monitor::all().context("list monitors")?;
        let monitor = monitors.into_iter().next().context("no monitors found")?;
        let scale_factor = monitor.scale_factor().unwrap_or(1.0).max(1.0);
        let origin = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
        Ok(ScreenCapture {
            monitor,
            scale_factor,
            origin,
        })
    }

    /// Capture a single full-resolution frame from the monitor.
    ///
    /// On Windows the OS cursor is composited onto the RGBA buffer here —
    /// xcap's WGC backend disables cursor capture, so without this step the
    /// stream never shows the host pointer. See `cursor_overlay_windows`.
    pub fn next_frame(&self) -> anyhow::Result<RgbaImage> {
        #[allow(unused_mut)]
        let mut img = self.monitor.capture_image().context("capture_image")?;
        #[cfg(target_os = "windows")]
        crate::cursor_overlay_windows::compose(&mut img, self.origin);
        Ok(img)
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

/// Wall-clock interval between captured frames at a given tier. Public so
/// the H.264 video pipeline's writer task can pace `track.write_sample`
/// to the same cadence — SCK can deliver in bursts, and a constant-rate
/// writer keeps Chrome's jitter buffer minimal.
pub fn frame_interval(tier: QualityTier) -> Duration {
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
    /// - `hidpi_mode`: when true, `quality_resize` skips the 1/scale_factor
    ///   normalisation step so the encoder receives native physical pixels.
    /// - `force_iframe_rx`: optional oneshot that, when fired, resets the tile
    ///   diff so the next emitted frame contains every tile (equivalent to an
    ///   H.264 IDR for a joining viewer).
    /// - `input_wake`: optional `Arc<Notify>` that breaks the idle-backoff
    ///   sleep when input arrives so the next frame ships at active cadence.
    ///   `None` keeps the legacy fixed-cadence behaviour.
    ///
    /// **Precondition:** Caller must verify `desktop::desktop_available()`
    /// is `true` before spawning this loop. We do not re-probe here — that
    /// would duplicate the TCC prompt on macOS first-run.
    pub fn run(
        tier: QualityTier,
        tx: Sender<FrameOutput>,
        scale_factor: f32,
        hidpi_mode: bool,
        force_iframe_rx: Option<oneshot::Receiver<()>>,
        input_wake: Option<InputWake>,
    ) {
        // ── macOS SCK fast path ─────────────────────────────────────────────
        // SCK delivers BGRA pre-sized at the encoder's tier dims via GPU
        // IOSurface (~3-5 ms vs xcap's ~28 ms). Skip the entire
        // `quality_resize` stage when SCK is available; xcap stays as
        // fallback for older macOS / permission denied / non-macOS.
        #[cfg(target_os = "macos")]
        if let Some((logical_w, logical_h)) = primary_dims() {
            let (target_w, target_h) =
                crate::encode::resize_dims(logical_w, logical_h, tier, scale_factor, hidpi_mode);
            let target_fps = max_tier_fps_hz();
            match crate::sck::SckCapture::new(target_w, target_h, target_fps) {
                Ok(sck) => {
                    info!(
                        tier = ?tier,
                        target_w, target_h, target_fps, hidpi_mode,
                        "CaptureLoop started (ScreenCaptureKit, JPEG)"
                    );
                    return run_jpeg_sck(sck, tier, tx, force_iframe_rx, input_wake);
                }
                Err(err) => warn!(
                    error = %err,
                    "ScreenCaptureKit init failed for JPEG path, falling back to xcap"
                ),
            }
        }

        // ── xcap fallback ────────────────────────────────────────────────────
        let mut force_iframe_rx = force_iframe_rx;
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
            "CaptureLoop started (xcap, JPEG)"
        );

        let active_interval = frame_interval(tier);
        let idle_max = Duration::from_millis(IDLE_INTERVAL_MAX_MS);
        let mut interval = active_interval;
        let mut idle_streak: u32 = 0;
        let mut diff = TileDiff::new();
        let mut frame_count: u64 = 0;
        let mut dropped_count: u64 = 0;

        // Tokio handle for input-wake select. Captured once up-front so the
        // hot loop doesn't pay the lookup cost.
        let tokio_handle = if input_wake.is_some() {
            tokio::runtime::Handle::try_current().ok()
        } else {
            None
        };

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
            let output =
                TileEncoder::process_frame(raw, tier, scale_factor, hidpi_mode, &mut diff);

            // Only emit non-idle frames. Drop-newest on backpressure so the
            // capture thread never stalls behind a slow consumer.
            let was_idle = output.tiles.is_empty();
            if !was_idle {
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

            // Adaptive backoff: 3 idle frames in a row → grow interval 1.5×
            // up to the idle ceiling. Any non-idle frame snaps back to active
            // cadence immediately.
            if was_idle {
                idle_streak = idle_streak.saturating_add(1);
                if idle_streak >= 3 {
                    let next_ms = (interval.as_millis() as u64).saturating_mul(3) / 2;
                    interval = Duration::from_millis(next_ms).min(idle_max);
                }
            } else {
                idle_streak = 0;
                interval = active_interval;
            }

            // Sleep the remaining budget for this frame interval. With
            // `input_wake` wired up we use a tokio select so a mouse jiggle
            // shortens the sleep — wake.notified() resets us to active.
            let elapsed = frame_start.elapsed();
            let Some(remaining) = interval.checked_sub(elapsed) else {
                continue;
            };

            let woken = match (&tokio_handle, &input_wake) {
                (Some(h), Some(wake)) => h.block_on(async {
                    tokio::select! {
                        _ = tokio::time::sleep(remaining) => false,
                        _ = wake.notified() => true,
                    }
                }),
                _ => {
                    std::thread::sleep(remaining);
                    false
                }
            };

            if woken {
                interval = active_interval;
                idle_streak = 0;
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
    ///
    /// `hidpi_mode` is also captured ONCE — like resolution it changes the
    /// encoder dims and so requires a session-level restart to flip. The
    /// caller (`desktop_ws.rs`) handles that path.
    #[allow(clippy::too_many_arguments)]
    pub fn run_bgra(
        resolution_tier: QualityTier,
        fps_rx: tokio::sync::watch::Receiver<QualityTier>,
        tx: Sender<RawBgraFrame>,
        scale_factor: f32,
        hidpi_mode: bool,
        force_iframe_rx: Option<oneshot::Receiver<()>>,
        // Phase-02a: when `Some`, the SCK backend opens the SCStream with
        // `with_captures_audio(true)` and forwards 20 ms i16 stereo PCM
        // frames out through this sender. xcap fallback ignores audio
        // (Linux + older macOS deferred per phase-02c plan). Passing
        // `None` preserves the pre-phase-02 video-only behaviour exactly.
        audio_tx: Option<Sender<Vec<i16>>>,
        // Observability counter — when Some, incremented per successful
        // `tx.try_send`. The H.264 session watchdog reads this at
        // fallback time to distinguish "capture never delivered a frame"
        // from "encoder rejected every frame". Pass None for callers that
        // don't need diagnostics.
        frames_sent: Option<Arc<AtomicU64>>,
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
            let (target_w, target_h) = crate::encode::resize_dims(
                screen_w,
                screen_h,
                resolution_tier,
                scale_factor,
                hidpi_mode,
            );
            // SCK target FPS is the ceiling; we still gate on fps_rx in the
            // hot loop. Pass the highest tier's FPS so SCK never throttles
            // below what the user might select mid-session.
            let target_fps = max_tier_fps_hz();
            if let Some(audio_tx) = audio_tx.clone() {
                // Audio path: open one SCStream for both video + audio so
                // BUNDLE'd A/V sync is structural at capture time. Spawn an
                // async forwarder for the audio frames since the blocking
                // capture loop below can't await an mpsc.
                match crate::sck::SckCapture::new_with_audio(target_w, target_h, target_fps) {
                    Ok((sck, mut audio_rx)) => {
                        info!(
                            resolution_tier = ?resolution_tier,
                            initial_fps_tier = ?*fps_rx.borrow(),
                            target_w, target_h, target_fps, hidpi_mode,
                            "CaptureLoop::run_bgra started (ScreenCaptureKit, +audio)"
                        );
                        if let Ok(handle) = tokio::runtime::Handle::try_current() {
                            handle.spawn(async move {
                                while let Some(frame) = audio_rx.recv().await {
                                    if audio_tx.send(frame).await.is_err() {
                                        // Consumer dropped the audio pipeline
                                        // (session closed). Drop the rx so the
                                        // SCK handler stops trying to send;
                                        // video keeps running.
                                        break;
                                    }
                                }
                            });
                        } else {
                            warn!(
                                "no tokio runtime handle inside spawn_blocking; \
                                 audio forwarder not started"
                            );
                        }
                        return run_bgra_sck(sck, fps_rx, tx, force_iframe_rx, frames_sent);
                    }
                    Err(err) => warn!(
                        error = %err,
                        "ScreenCaptureKit (audio) init failed; trying video-only SCK"
                    ),
                }
            }
            match crate::sck::SckCapture::new(target_w, target_h, target_fps) {
                Ok(sck) => {
                    info!(
                        resolution_tier = ?resolution_tier,
                        initial_fps_tier = ?*fps_rx.borrow(),
                        target_w, target_h, target_fps, hidpi_mode,
                        "CaptureLoop::run_bgra started (ScreenCaptureKit)"
                    );
                    return run_bgra_sck(sck, fps_rx, tx, force_iframe_rx, frames_sent);
                }
                Err(err) => warn!(error = %err, "ScreenCaptureKit init failed, falling back to xcap"),
            }
        }
        // xcap fallback: audio is not supported on this path. Drop the sender
        // so the consumer's recv returns None and the audio pipeline shuts
        // down cleanly instead of hanging.
        let _ = audio_tx;

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
            scale_factor, hidpi_mode,
            "CaptureLoop::run_bgra started (xcap)"
        );

        run_bgra_xcap(
            capture,
            resolution_tier,
            fps_rx,
            tx,
            scale_factor,
            hidpi_mode,
            force_iframe_rx,
            frames_sent,
        );
    }
}

// ── Backend-specific drivers ─────────────────────────────────────────────────

/// xcap-backed driver. Captures full physical RGBA, runs `quality_resize` +
/// `rgba_to_bgra`, then forwards a `RawBgraFrame`.
#[allow(clippy::too_many_arguments)]
fn run_bgra_xcap(
    capture: ScreenCapture,
    resolution_tier: QualityTier,
    fps_rx: tokio::sync::watch::Receiver<QualityTier>,
    tx: Sender<RawBgraFrame>,
    scale_factor: f32,
    hidpi_mode: bool,
    mut force_iframe_rx: Option<oneshot::Receiver<()>>,
    frames_sent: Option<Arc<AtomicU64>>,
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

        let resized = quality_resize(raw, resolution_tier, scale_factor, hidpi_mode);
        let (width, height) = (resized.width(), resized.height());
        let bytes = rgba_to_bgra(resized.as_raw());

        match tx.try_send(RawBgraFrame {
            bytes,
            width,
            height,
            force_idr,
            // xcap doesn't surface dirty rects — full-frame active-map
            // every tick (no encode speed-up vs status quo on Linux/Windows
            // until phase-03c plumbs DXGI dirty rects).
            dirty_rects: Vec::new(),
        }) {
            Ok(()) => {
                frame_count += 1;
                if let Some(c) = &frames_sent {
                    c.fetch_add(1, Ordering::Relaxed);
                }
                if frame_count == 1 {
                    info!(width, height, "CaptureLoop::run_bgra (xcap) first frame delivered");
                }
            }
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
    frames_sent: Option<Arc<AtomicU64>>,
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

        // 2026-05-16: discard SCK's dirty_rects at the pipeline boundary.
        //
        // `CMSampleBuffer.dirty_rects()` reports framebuffer redraws, but the
        // macOS cursor is composited AFTER framebuffer dirty tracking — so a
        // pure cursor move (no underlying window redraw) yields dirty rects
        // that exclude both the cursor's old and new bounding box. The
        // cursor pixels ARE in the BGRA output (SCK composites them on
        // output), but downstream active-map encoders (VP9 `VP8E_SET_ACTIVEMAP`,
        // AV1 `AOME_SET_ACTIVEMAP`) then mark those tiles INACTIVE → encoder
        // SKIPs them → decoder reuses the previous frame's cursor pixels →
        // visible cursor history trail across the screen.
        //
        // `encoders::Vp9Encoder::apply_dirty_rects` doc-contract: "empty rects
        // with force_full=false means 'no info — assume full'", so passing
        // `Vec::new()` makes the encoder mark every block active. Matches the
        // xcap path which has never had this bug. Trades the active-map CPU
        // optimization on idle SCK frames for correctness — the optimization
        // helped libvpx/libaom but didn't apply to VideoToolbox H.264 anyway
        // (the dominant pipeline on macOS).
        let _ = frame.dirty_rects;
        match tx.try_send(RawBgraFrame {
            bytes: frame.bytes,
            width: frame.width,
            height: frame.height,
            force_idr,
            dirty_rects: Vec::new(),
        }) {
            Ok(()) => {
                frame_count += 1;
                if let Some(c) = &frames_sent {
                    c.fetch_add(1, Ordering::Relaxed);
                }
                if frame_count == 1 {
                    info!("CaptureLoop::run_bgra (sck) first frame delivered");
                }
            }
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

/// JPEG-on-SCK driver. SCK delivers BGRA pre-sized at the encoder's tier
/// dims via GPU IOSurface; we hand the bytes straight to `process_frame_bgra`
/// (no `quality_resize`, no RGBA→BGRA convert).
///
/// The input-wake `Arc<Notify>` from the WS layer isn't consumed here —
/// SCK pushes frames at its configured cadence, so there is no sleep for
/// us to interrupt. The tier gate + idle backoff naturally re-engage on
/// the next frame after input reaches the encoder.
#[cfg(target_os = "macos")]
fn run_jpeg_sck(
    mut sck: crate::sck::SckCapture,
    tier: QualityTier,
    tx: Sender<FrameOutput>,
    mut force_iframe_rx: Option<oneshot::Receiver<()>>,
    _input_wake: Option<InputWake>,
) {
    use crate::encode::{TileDiff, TileEncoder, IDLE_INTERVAL_MAX_MS};

    let active_interval = frame_interval(tier);
    let idle_max = Duration::from_millis(IDLE_INTERVAL_MAX_MS);
    let mut interval = active_interval;
    let mut idle_streak: u32 = 0;
    let mut diff = TileDiff::new();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut frame_count: u64 = 0;
    let mut dropped_count: u64 = 0;

    loop {
        // Force-IDR poll mirrors the xcap path so a new viewer joining
        // mid-session always sees a complete frame.
        if let Some(mut rx) = force_iframe_rx.take() {
            match rx.try_recv() {
                Ok(()) => {
                    diff.reset();
                    info!("run_jpeg_sck: force-iframe received, next frame is full");
                }
                Err(oneshot::error::TryRecvError::Empty) => force_iframe_rx = Some(rx),
                Err(oneshot::error::TryRecvError::Closed) => {}
            }
        }

        let frame = match sck.next_frame_blocking() {
            Some(f) => f,
            None => {
                info!("run_jpeg_sck: SCK stream ended");
                break;
            }
        };

        // Tier gate: SCK runs at its max configured cadence; drop frames
        // arriving sooner than the tier interval. Idle backoff ramps
        // `interval` up to 400 ms when the screen is static.
        let now = Instant::now();
        if now.duration_since(last_emit) < interval {
            continue;
        }
        last_emit = now;

        let output = TileEncoder::process_frame_bgra(
            &frame.bytes,
            frame.width,
            frame.height,
            tier,
            &mut diff,
        );
        let was_idle = output.tiles.is_empty();
        if !was_idle {
            match tx.try_send(output) {
                Ok(()) => frame_count += 1,
                Err(TrySendError::Full(_)) => {
                    dropped_count += 1;
                    tracing::debug!(dropped = dropped_count, "run_jpeg_sck: backpressure drop");
                }
                Err(TrySendError::Closed(_)) => {
                    info!("run_jpeg_sck: channel closed, stopping");
                    break;
                }
            }
        }

        if was_idle {
            idle_streak = idle_streak.saturating_add(1);
            if idle_streak >= 3 {
                let next_ms = (interval.as_millis() as u64).saturating_mul(3) / 2;
                interval = Duration::from_millis(next_ms).min(idle_max);
            }
        } else {
            idle_streak = 0;
            interval = active_interval;
        }
    }

    info!(
        frames_sent = frame_count,
        frames_dropped = dropped_count,
        "run_jpeg_sck stopped"
    );
}

/// Look up the primary monitor's logical pixel dimensions. Used to decide
/// SCK output dims — see `run_bgra` and the JPEG SCK fast path.
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


/// Pixel-space rectangle. Used to surface SCK / DXGI dirty rects so the
/// VP9 / AV1 encoders can drive their active-map and skip unchanged
/// 16×16 blocks (the #1 speed gap vs Chrome Remote Desktop). Coordinates
/// are in the same logical/tier pixel space as `RawBgraFrame.bytes`.
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// One captured frame already resized to the logical/tier dimensions, ready
/// for H.264/VP9/AV1 encode. Byte order is BGRA (matches
/// `kCVPixelFormatType_32BGRA` and the `yuvutils-rs` BGRA→I420 path).
#[derive(Debug)]
pub struct RawBgraFrame {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Set when the pipeline must mark the next encoded frame as an IDR.
    /// Driven by client PLI over RTCP or by session-open cold-start.
    pub force_idr: bool,
    /// Phase-03b: 16×16-block active-map driver. Empty `Vec` means "every
    /// block is dirty" (capture backend doesn't expose dirty rects, or the
    /// frame is the session's cold-start full frame). VP9 + AV1 encoders
    /// consume this; H.264 + JPEG ignore it.
    pub dirty_rects: Vec<DirtyRect>,
}

/// Convert an RGBA8 byte slice in-place-style to a BGRA8 Vec. xcap delivers
/// RGBA; both our H.264 encoder backends (VT's `kCVPixelFormatType_32BGRA`
/// and OpenH264's `yuvutils-rs::bgra_to_yuv420`) take BGRA.
///
/// Per-pixel swap of R↔B. Could be SIMD-accelerated later; at typical tier
/// resolutions (≤ 1512×982 logical) this is ~1.5 M pixels ≈ < 3 ms on an
/// M-series core — well under the 33 ms frame budget.
fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    debug_assert!(rgba.len().is_multiple_of(4));
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
