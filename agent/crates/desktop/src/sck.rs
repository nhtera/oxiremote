//! ScreenCaptureKit (SCK) backend — macOS 12.3+ low-latency screen capture.
//!
//! Replaces xcap's `CGWindowListCreateImage` hot path on supported hosts.
//! SCK delivers pre-resized BGRA frames via a GPU `IOSurface`, lifting capture
//! cost from ~28 ms / frame (CGDisplayCreateImage on Apple Silicon Retina) to
//! a few milliseconds and unblocking the path to true 30 fps + sub-100 ms
//! glass-to-glass latency.
//!
//! Architecture:
//! - One `SCStream` per session, configured at the encoder's locked
//!   resolution. SCK does the GPU downscale; we never see physical-resolution
//!   pixels, so neither `quality_resize` nor `rgba_to_bgra` runs on the SCK
//!   path. The encoder's BGRA buffer is essentially a memcpy from the GPU.
//! - The SCK output handler runs on an internal Apple dispatch queue. We
//!   marshal frames over a bounded `tokio::sync::mpsc(1)` channel to the
//!   capture thread (`CaptureLoop::run_bgra`). Cap=1 + `try_send` keeps the
//!   queue from filling — at high motion the consumer always sees the
//!   freshest frame, never a backlog.
//! - Construction failure (older macOS, missing Screen Recording permission,
//!   etc.) returns `Err` so the caller can fall back to xcap. The ad-hoc
//!   `SCShareableContent::get()` round-trip we do at startup also surfaces
//!   permission errors loud enough to log clearly.

#![cfg(target_os = "macos")]

use anyhow::{Context, Result, anyhow};
use screencapturekit::cm::CMTime;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use tokio::sync::mpsc;
use tracing::warn;

/// One BGRA frame ready for the H.264 encoder. SCK has already resized to
/// the configured stream dimensions; the consumer can hand this straight
/// to VideoToolbox without an intermediate `RgbaImage` allocation.
pub struct SckFrame {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Owning handle for an active SCK capture session. Drop stops the stream.
pub struct SckCapture {
    rx: mpsc::Receiver<SckFrame>,
    // SCStream's destructor stops the underlying capture; keeping it alive
    // here ties the stream's lifetime to ours.
    _stream: SCStream,
}

struct Handler {
    tx: mpsc::Sender<SckFrame>,
}

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, _type: SCStreamOutputType) {
        // Non-Screen sample types or invalid frames are common at startup;
        // skip them silently.
        if !sample.is_valid() {
            return;
        }
        let Some(pixel_buffer) = sample.image_buffer() else {
            return;
        };
        let w = pixel_buffer.width() as u32;
        let h = pixel_buffer.height() as u32;
        if w == 0 || h == 0 {
            return;
        }
        let row = pixel_buffer.bytes_per_row();
        let expected = (w as usize) * 4;
        let Ok(guard) = pixel_buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
            return;
        };
        let base = guard.base_address();
        if base.is_null() {
            return;
        }
        // SCK can return frames with row stride > width*4 when the GPU
        // surface is hardware-aligned. We strip padding into a tightly-packed
        // BGRA buffer so the encoder's input stride matches its width.
        let bytes = if row == expected {
            // SAFETY: base is valid for h*row bytes per CVPixelBuffer contract.
            unsafe { std::slice::from_raw_parts(base, (h as usize) * row).to_vec() }
        } else {
            let mut out = Vec::with_capacity((h as usize) * expected);
            for r in 0..h as usize {
                // SAFETY: each row is `row` bytes; we copy the first `expected`.
                let row_slice =
                    unsafe { std::slice::from_raw_parts(base.add(r * row), expected) };
                out.extend_from_slice(row_slice);
            }
            out
        };
        // Cap=1 + try_send: drop frames that can't be delivered, never block
        // SCK's internal dispatch queue. Backpressure here means the consumer
        // is slow; landing the freshest frame later is better than queueing.
        let _ = self.tx.try_send(SckFrame { bytes, width: w, height: h });
    }
}

impl SckCapture {
    /// Construct an SCK stream targeting the primary display at the given
    /// output dimensions and frame rate.
    ///
    /// Returns `Err` on:
    /// - macOS < 12.3 (SCK is not available)
    /// - Missing Screen Recording permission (SCShareableContent::get fails)
    /// - Any other ScreenCaptureKit init error
    pub fn new(width: u32, height: u32, target_fps: u32) -> Result<Self> {
        let content = SCShareableContent::get()
            .map_err(|e| anyhow!("SCShareableContent::get failed (Screen Recording permission?): {e}"))?;
        let displays = content.displays();
        let display = displays
            .first()
            .ok_or_else(|| anyhow!("ScreenCaptureKit: no displays available"))?;
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();

        // CMTime numerator/denominator: 1/fps seconds. We use a 600 timescale
        // (Apple's recommended default) and compute value = 600 / fps.
        let interval = CMTime::new((600 / target_fps.max(1)) as i64, 600);
        let config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_pixel_format(PixelFormat::BGRA)
            .with_minimum_frame_interval(&interval);

        let (tx, rx) = mpsc::channel(1);
        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(Handler { tx }, SCStreamOutputType::Screen);
        stream
            .start_capture()
            .context("ScreenCaptureKit: start_capture failed")?;
        tracing::info!(
            width,
            height,
            target_fps,
            "ScreenCaptureKit stream started"
        );
        Ok(Self { rx, _stream: stream })
    }

    /// Block the caller until the next BGRA frame arrives. Returns `None` if
    /// the stream has been torn down (handler-side `tx` was dropped).
    pub fn next_frame_blocking(&mut self) -> Option<SckFrame> {
        self.rx.blocking_recv()
    }

    /// Non-blocking poll. Useful for periodic shutdown checks alongside a
    /// separate sleep schedule.
    #[allow(dead_code)]
    pub fn try_next_frame(&mut self) -> Option<SckFrame> {
        match self.rx.try_recv() {
            Ok(f) => Some(f),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                warn!("ScreenCaptureKit stream channel disconnected");
                None
            }
        }
    }
}
