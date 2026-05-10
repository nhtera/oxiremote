//! macOS Core Audio loopback (phase-02 stub).
//!
//! Per phase-02 plan, macOS audio is deferred — system loopback requires
//! either the BlackHole virtual device, a system-extension entitlement, or
//! ScreenCaptureKit's audio surface (still maturing). Returns `Unsupported`
//! so the capability endpoint reports `audio_supported=false` and the SPA
//! greys out the toggle on macOS hosts.

use super::{AudioCapture, AudioError};

pub fn make() -> Result<Box<dyn AudioCapture>, AudioError> {
    Err(AudioError::Unsupported("audio.platform.macos"))
}
