//! ScreenCaptureKit (SCK) backend — macOS 12.3+ low-latency screen + audio capture.
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
//!   marshal frames over bounded `tokio::sync::mpsc` channels to the capture
//!   thread (`CaptureLoop::run_bgra`) and (when audio is enabled) the audio
//!   pipeline. Video cap=1 + `try_send` keeps the queue from filling — at
//!   high motion the consumer always sees the freshest frame, never a
//!   backlog. Audio cap=8 (~160 ms) absorbs encoder jitter without dropping
//!   audible chunks under brief stalls.
//! - Construction failure (older macOS, missing Screen Recording permission,
//!   etc.) returns `Err` so the caller can fall back to xcap. The ad-hoc
//!   `SCShareableContent::get()` round-trip we do at startup also surfaces
//!   permission errors loud enough to log clearly.
//!
//! Audio path (phase-02a):
//! - When `new_with_audio()` is used, the same SCStream is configured with
//!   `with_captures_audio(true) + with_sample_rate(48000) + with_channel_count(2)`
//!   so video and audio share timestamps for free A/V sync.
//! - SCK delivers audio as **planar Float32** (one `AudioBuffer` per channel:
//!   LLLL..., RRRR...). The handler interleaves to LRLR and clamps Float32
//!   to i16 before pushing — that matches the wire format `audiopus`
//!   expects, so downstream `OpusEncoder::encode_frame` consumes the buffers
//!   1:1 with no extra reshape.
//! - Each SCK audio callback delivers exactly 960 frames (= 20 ms @ 48 kHz),
//!   matching the Opus frame size. One callback → one Opus frame.

#![cfg(target_os = "macos")]

use anyhow::{Context, Result, anyhow};
use screencapturekit::cm::{CMSampleBuffer, CMTime};
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
    /// Phase-03b: SCK-reported changed regions for this frame, mapped from
    /// `SCStreamFrameInfo.dirtyRects` via `screencapturekit 1.5`'s
    /// `CMSampleBuffer::dirty_rects()`. Coordinates are in the SCK stream's
    /// pixel space, which matches `width × height` above. Empty when SCK
    /// returned no dirty-rect attachment (rare; treat as full-frame).
    pub dirty_rects: Vec<crate::capture::DirtyRect>,
}

/// Stereo PCM samples ready for `OpusEncoder::encode_frame`. Interleaved
/// LRLR i16, sized to exactly 20 ms @ 48 kHz (1920 samples = 960 stereo
/// frames).
pub type SckAudioFrame = Vec<i16>;

/// Audio mpsc capacity. Each slot is one 20 ms Opus frame, so 8 ≈ 160 ms of
/// buffered audio — generous enough to absorb encoder jitter without
/// accumulating perceptible latency on the consumer side.
const AUDIO_CHANNEL_CAP: usize = 8;

/// Opus / SCK shared wire format: 48 kHz stereo, 20 ms frames.
const AUDIO_SAMPLE_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u32 = 2;

/// Owning handle for an active SCK capture session. Drop stops the stream.
pub struct SckCapture {
    rx: mpsc::Receiver<SckFrame>,
    // SCStream's destructor stops the underlying capture; keeping it alive
    // here ties the stream's lifetime to ours.
    _stream: SCStream,
}

struct Handler {
    video_tx: mpsc::Sender<SckFrame>,
    /// Some when the SCStream is configured with audio; the same Handler is
    /// re-used for both output kinds because SCK invokes one callback per
    /// `SCStreamOutputType::{Screen, Audio}` registration.
    audio_tx: Option<mpsc::Sender<SckAudioFrame>>,
}

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !sample.is_valid() {
            return;
        }
        match kind {
            SCStreamOutputType::Screen => self.on_video(sample),
            SCStreamOutputType::Audio => {
                if let Some(tx) = self.audio_tx.as_ref() {
                    on_audio(tx, sample);
                }
            }
            // Other output kinds (Microphone) are not registered; silently ignore.
            _ => {}
        }
    }
}

impl Handler {
    fn on_video(&self, sample: CMSampleBuffer) {
        // Non-Screen sample types or invalid frames are common at startup;
        // skip them silently.
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
        // Phase-03b: extract dirty rects from SCStreamFrameInfo so the
        // active-map encoders can skip unchanged 16×16 blocks. SCK returns
        // CGRect in pixel space (same coordinate basis as `bytes`); cast
        // through clamping into u32. None / empty falls back to full-frame.
        let dirty_rects: Vec<crate::capture::DirtyRect> = sample
            .dirty_rects()
            .map(|rects| {
                rects
                    .into_iter()
                    .filter_map(|r| {
                        let x = r.x.max(0.0) as u32;
                        let y = r.y.max(0.0) as u32;
                        let width = r.width.max(0.0) as u32;
                        let height = r.height.max(0.0) as u32;
                        if width == 0 || height == 0 {
                            None
                        } else {
                            Some(crate::capture::DirtyRect { x, y, width, height })
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Cap=1 + try_send: drop frames that can't be delivered, never block
        // SCK's internal dispatch queue. Backpressure here means the consumer
        // is slow; landing the freshest frame later is better than queueing.
        let _ = self.video_tx.try_send(SckFrame { bytes, width: w, height: h, dirty_rects });
    }
}

/// Convert one SCK audio CMSampleBuffer to an interleaved LRLR i16 PCM frame
/// and hand it to the audio mpsc.
///
/// SCK delivers planar Float32 (format_flags = Float | Packed | NonInterleaved,
/// 0x29), one `AudioBuffer` per channel. We:
///   1. Read the planar L/R buffers from `audio_buffer_list()`.
///   2. Interleave samples pairwise (`out[2i]=L[i], out[2i+1]=R[i]`).
///   3. Clamp Float32 `[-1.0, 1.0]` → i16 `[-32767, 32767]`.
///
/// The result is exactly the wire format `audiopus::OpusEncoder` consumes,
/// so the downstream pipeline does not need a reshape step.
fn on_audio(tx: &mpsc::Sender<SckAudioFrame>, sample: CMSampleBuffer) {
    let Some(list) = sample.audio_buffer_list() else {
        return;
    };
    let frames = sample.num_samples();
    if frames == 0 {
        return;
    }

    // Collect channel planes. SCK delivers planar → one buffer per channel.
    // Some hosts may emit mono in a single buffer; upmix by duplication.
    let mut planes: Vec<&[f32]> = Vec::with_capacity(2);
    for buf in list.iter() {
        let raw = buf.data();
        // Each plane is `frames * 4` bytes of f32. Anything shorter is
        // malformed; skip the whole sample buffer rather than guess.
        if raw.len() < frames * std::mem::size_of::<f32>() {
            return;
        }
        // SAFETY: SCK guarantees the buffer is well-aligned and sized for
        // `frames` f32 samples on macOS (CoreAudio contract).
        let plane = unsafe {
            std::slice::from_raw_parts(raw.as_ptr() as *const f32, frames)
        };
        planes.push(plane);
        if planes.len() == 2 {
            break;
        }
    }
    if planes.is_empty() {
        return;
    }

    let mut out = Vec::with_capacity(frames * 2);
    let left = planes[0];
    let right = if planes.len() >= 2 { planes[1] } else { planes[0] };
    for i in 0..frames {
        out.push(f32_to_i16(left[i]));
        out.push(f32_to_i16(right[i]));
    }

    // Drop-newest under backpressure: if the encoder/writer stall and the
    // 8-slot ring fills, dropping a 20 ms frame is preferable to blocking
    // SCK's dispatch queue (which would also stall video).
    let _ = tx.try_send(out);
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

impl SckCapture {
    /// Construct an SCK stream targeting the primary display at the given
    /// output dimensions and frame rate (video only).
    ///
    /// Returns `Err` on:
    /// - macOS < 12.3 (SCK is not available)
    /// - Missing Screen Recording permission (SCShareableContent::get fails)
    /// - Any other ScreenCaptureKit init error
    pub fn new(width: u32, height: u32, target_fps: u32) -> Result<Self> {
        let (video_tx, video_rx) = mpsc::channel(1);
        let stream = build_stream(width, height, target_fps, /*with_audio=*/ false)?;
        let handler = Handler { video_tx, audio_tx: None };
        register_and_start(stream, handler, /*with_audio=*/ false)
            .map(|stream| Self { rx: video_rx, _stream: stream })
    }

    /// Construct an SCK stream that emits both BGRA video frames and 20 ms
    /// Opus-ready audio frames over independent mpsc receivers. Audio and
    /// video share the underlying CoreMedia timestamps so A/V sync is
    /// structurally aligned at capture time.
    ///
    /// Returns `(SckCapture, audio_rx)` where the caller hands `audio_rx` to
    /// `SckAudioCapture::from_receiver` (audio/sck_audio.rs).
    pub fn new_with_audio(
        width: u32,
        height: u32,
        target_fps: u32,
    ) -> Result<(Self, mpsc::Receiver<SckAudioFrame>)> {
        let (video_tx, video_rx) = mpsc::channel(1);
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_CHANNEL_CAP);
        let stream = build_stream(width, height, target_fps, /*with_audio=*/ true)?;
        let handler = Handler { video_tx, audio_tx: Some(audio_tx) };
        let stream = register_and_start(stream, handler, /*with_audio=*/ true)?;
        Ok((Self { rx: video_rx, _stream: stream }, audio_rx))
    }

    /// Block the caller until the next BGRA frame arrives. Returns `None` if
    /// the stream has been torn down (handler-side `tx` was dropped).
    ///
    /// Superseded by the timeout-wrapped [`recv`](Self::recv) in the capture
    /// loops (which can observe consumer-drop on a static screen); kept for
    /// callers that want an unconditional block.
    #[allow(dead_code)]
    pub fn next_frame_blocking(&mut self) -> Option<SckFrame> {
        self.rx.blocking_recv()
    }

    /// Async receive — await the next BGRA frame. Returns `None` when the
    /// stream is torn down. Lets the capture loop wrap the wait in a
    /// `tokio::time::timeout` so it can periodically check whether its
    /// downstream consumer is gone. Without this the loop parks forever in
    /// `next_frame_blocking` when SCK stops delivering frames on a static
    /// screen, leaking the SCStream (and the whole encode pipeline behind it)
    /// on every session that ends while the host screen is idle.
    pub async fn recv(&mut self) -> Option<SckFrame> {
        self.rx.recv().await
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

fn build_stream(
    width: u32,
    height: u32,
    target_fps: u32,
    with_audio: bool,
) -> Result<SCStream> {
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
    let mut config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_pixel_format(PixelFormat::BGRA)
        .with_minimum_frame_interval(&interval);
    if with_audio {
        config = config
            .with_captures_audio(true)
            .with_sample_rate(AUDIO_SAMPLE_RATE as i32)
            .with_channel_count(AUDIO_CHANNELS as i32)
            // Belt-and-braces: prevent the agent's own audio output from
            // looping back into the capture. We never play audio, but the
            // safer default protects against future shells / TUI sounds.
            .with_excludes_current_process_audio(true);
    }

    Ok(SCStream::new(&filter, &config))
}

fn register_and_start(
    mut stream: SCStream,
    handler: Handler,
    with_audio: bool,
) -> Result<SCStream> {
    if with_audio {
        // The same handler instance is registered twice — once per output
        // kind — so the closure can multiplex dispatch on `SCStreamOutputType`
        // without two struct copies. Cloning the senders is cheap (`Arc`).
        let audio_handler = Handler {
            video_tx: handler.video_tx.clone(),
            audio_tx: handler.audio_tx.clone(),
        };
        stream.add_output_handler(handler, SCStreamOutputType::Screen);
        stream.add_output_handler(audio_handler, SCStreamOutputType::Audio);
    } else {
        stream.add_output_handler(handler, SCStreamOutputType::Screen);
    }
    stream
        .start_capture()
        .context("ScreenCaptureKit: start_capture failed")?;
    tracing::info!(with_audio, "ScreenCaptureKit stream started");
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_clamps_extremes() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX); // -32767, not -32768 — symmetric
        assert_eq!(f32_to_i16(2.5), i16::MAX, "values above 1.0 must clamp");
        assert_eq!(f32_to_i16(-2.5), -i16::MAX, "values below -1.0 must clamp");
    }

    #[test]
    fn f32_to_i16_mid_scale() {
        // 0.5 → ~half of i16::MAX (16383).
        let half = f32_to_i16(0.5);
        assert!((16000..=16500).contains(&half), "0.5 should map near 16383, got {half}");
    }
}
