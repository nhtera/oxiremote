//! Linux PulseAudio monitor-source loopback (phase-02 stub).
//!
//! Off-persona for v1 (iPad → Windows host is the target). Returns
//! `Unsupported` so the capability endpoint reports `audio_supported=false`
//! on Linux hosts. Real impl could pull from a `pulse_audio_monitor` source
//! via cpal once a Linux user requests it.

use super::{AudioCapture, AudioError};

pub fn make() -> Result<Box<dyn AudioCapture>, AudioError> {
    Err(AudioError::Unsupported("audio.platform.linux"))
}
