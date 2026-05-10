//! Error types for the audio capture / encode pipeline.
//!
//! Phase-02 scaffold. Concrete capture impls land after the cpal WASAPI
//! kill-switch smoke passes on Win10/Win11. Until then, every platform's
//! capture factory returns `AudioError::Unsupported(reason)` and consumers
//! treat the audio capability as off.

use std::fmt;

#[derive(Debug)]
pub enum AudioError {
    /// Platform / build configuration cannot capture audio at all (no impl,
    /// kill-switch not yet passed, or feature not compiled in). The string
    /// is a short reason code suitable for logging — `audio.unsupported`,
    /// `audio.platform.macos`, `audio.killswitch.pending`, etc.
    Unsupported(&'static str),
    /// Capture device could not be opened (permission denied, device gone).
    DeviceOpen(String),
    /// Mid-session capture failure. Consumers map this to `SignalOut::AudioLost`
    /// and stop the audio task without tearing down video.
    CaptureFailed(String),
    /// Opus encoder rejected a frame (wrong frame size, internal libopus error).
    EncodeFailed(String),
    /// Resampler could not reshape input to the target rate / channel count.
    ResampleFailed(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "audio capture unsupported: {reason}"),
            Self::DeviceOpen(msg) => write!(f, "audio device open failed: {msg}"),
            Self::CaptureFailed(msg) => write!(f, "audio capture failed: {msg}"),
            Self::EncodeFailed(msg) => write!(f, "opus encode failed: {msg}"),
            Self::ResampleFailed(msg) => write!(f, "audio resample failed: {msg}"),
        }
    }
}

impl std::error::Error for AudioError {}
