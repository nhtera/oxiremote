//! Windows WASAPI loopback capture (phase-02 stub).
//!
//! KILL-SWITCH GATE: real impl blocked on the cpal WASAPI loopback smoke test
//! on Win10 + Win11 (phase-02 step 1). Until that gate passes, `make()`
//! returns `AudioError::Unsupported("audio.killswitch.pending")` so the
//! capability endpoint reports `audio_supported=false` honestly and the SPA
//! greys out the toggle.
//!
//! When the kill-switch passes, this module will own:
//!   - `cpal::Stream` opened against the default output device in loopback /
//!     shared mode
//!   - 20 ms (960-sample @ 48 kHz) framing into an `mpsc::channel` (cap 8,
//!     drop-oldest under backpressure for low-latency interactive audio)
//!   - device-rate → 48 kHz stereo resampling via `super::resample` before
//!     handoff to `super::opus_encoder`

use super::{AudioCapture, AudioError};

/// Returns `Unsupported` until the WASAPI kill-switch passes. The reason
/// code lets log readers grep for the gate without parsing prose.
pub fn make() -> Result<Box<dyn AudioCapture>, AudioError> {
    Err(AudioError::Unsupported("audio.killswitch.pending"))
}
