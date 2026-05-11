//! macOS ScreenCaptureKit audio capture (phase-02a).
//!
//! SCK couples audio with video: a single `SCStream` configured for both
//! emits screen frames AND system audio over the same dispatch queue with
//! shared timestamps, giving us A/V sync for free. The video setup
//! (`crate::sck::SckCapture::new_with_audio`) owns the SCStream and hands
//! back a `mpsc::Receiver<Vec<i16>>` carrying 20 ms interleaved-stereo
//! frames at 48 kHz — exactly what `OpusEncoder::encode_frame` consumes.
//!
//! `SckAudioCapture` is a thin async wrapper over that receiver. The trait
//! impl exists so `desktop_audio_pipeline.rs` can treat macOS and (future)
//! Windows captures identically.
//!
//! Why no `make_default()` here? On macOS the audio capture can't be
//! constructed independently of the video stream — they share the SCStream.
//! `audio::mod::make_default()` therefore returns `Unsupported` on macOS
//! with reason `audio.macos.coupled-to-sck`; the actual instance is built
//! by the video setup path and handed into the audio pipeline directly.

#![cfg(target_os = "macos")]

use std::sync::OnceLock;

use async_trait::async_trait;
use screencapturekit::prelude::SCShareableContent;
use tokio::sync::mpsc;

use super::{AudioCapture, AudioError};

/// Sample rate of frames produced by `crate::sck::SckCapture::new_with_audio`.
pub const SAMPLE_RATE: u32 = 48_000;

/// Channel count (stereo, interleaved LRLR).
pub const CHANNELS: u16 = 2;

/// Sentinel: does this host have Screen Recording permission + a display SCK
/// can target? Cached for the binary lifetime — the underlying permission
/// can't change without a process restart, so the answer is stable.
///
/// Intentionally cheap (no `start_capture`): a full functional probe would
/// briefly trigger the macOS Recording Indicator on every cold-cache call,
/// which is hostile UX for an endpoint the SPA polls. macOS 12.x edge case
/// (SCK exists but audio doesn't): probe returns true and the actual
/// `SckCapture::new_with_audio` will surface the failure on first session,
/// which the audio pipeline reports as `AudioStopped { reason: "sck_error" }`.
pub fn probe_supported() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| match SCShareableContent::get() {
        Ok(content) => !content.displays().is_empty(),
        Err(err) => {
            tracing::debug!(error = %err, "sck audio probe: SCShareableContent::get failed");
            false
        }
    })
}

pub struct SckAudioCapture {
    rx: mpsc::Receiver<Vec<i16>>,
}

impl SckAudioCapture {
    /// Wrap the audio receiver returned alongside an `SckCapture` from
    /// `crate::sck::SckCapture::new_with_audio`. The receiver must be
    /// configured for 20 ms interleaved-stereo i16 frames at 48 kHz; any
    /// other shape will be rejected by `OpusEncoder::encode_frame`
    /// downstream, not here.
    pub fn from_receiver(rx: mpsc::Receiver<Vec<i16>>) -> Self {
        Self { rx }
    }
}

#[async_trait]
impl AudioCapture for SckAudioCapture {
    async fn next_frame(&mut self) -> Result<Vec<i16>, AudioError> {
        match self.rx.recv().await {
            Some(frame) => Ok(frame),
            None => Err(AudioError::CaptureFailed(
                "sck audio channel closed".to_string(),
            )),
        }
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn yields_frame_when_sender_pushes() {
        let (tx, rx) = mpsc::channel(2);
        let mut cap = SckAudioCapture::from_receiver(rx);
        let frame: Vec<i16> = (0..1920).map(|i| i as i16).collect();
        tx.send(frame.clone()).await.expect("send");
        let got = cap.next_frame().await.expect("recv");
        assert_eq!(got, frame);
        assert_eq!(cap.sample_rate(), 48_000);
        assert_eq!(cap.channels(), 2);
    }

    #[tokio::test]
    async fn errors_when_channel_closes() {
        let (tx, rx) = mpsc::channel::<Vec<i16>>(1);
        drop(tx);
        let mut cap = SckAudioCapture::from_receiver(rx);
        match cap.next_frame().await {
            Err(AudioError::CaptureFailed(msg)) => assert!(msg.contains("sck audio channel")),
            other => panic!("expected CaptureFailed, got {other:?}"),
        }
    }
}
