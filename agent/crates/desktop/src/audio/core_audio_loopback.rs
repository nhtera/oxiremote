//! macOS audio factory shim.
//!
//! Phase-02a chose ScreenCaptureKit (`super::sck_audio`) for macOS system
//! audio capture because SCK delivers audio + video over the same SCStream
//! with shared timestamps — A/V sync is structural rather than negotiated.
//! That coupling means the audio capture cannot be opened independently of
//! the video stream, so `make()` here returns `Unsupported` with a stable
//! reason code; the actual `SckAudioCapture` is constructed by the video
//! setup path (`crate::sck::SckCapture::new_with_audio`) and handed into
//! the audio pipeline directly.
//!
//! Kept as a file (rather than deleted) so `audio::mod`'s
//! `cfg(target_os = "macos")` factory has a stable call site that doesn't
//! churn when the wiring lands. Once the audio pipeline ships, callers will
//! never invoke this — the trait factory becomes a Windows-only path.

use super::{AudioCapture, AudioError};

pub fn make() -> Result<Box<dyn AudioCapture>, AudioError> {
    Err(AudioError::Unsupported("audio.macos.coupled-to-sck"))
}
