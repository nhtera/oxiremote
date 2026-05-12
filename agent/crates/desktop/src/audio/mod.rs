//! Audio capture + encode pipeline (phase-02 scaffold).
//!
//! Surface contract:
//!   - `AudioCapture` trait: pull-based, async `next_frame()` that yields
//!     interleaved i16 PCM at the device's native sample rate / channel count.
//!     Resampling to the wire format (48 kHz stereo) happens upstream in
//!     `resample`; encode in `opus_encoder`.
//!   - `make_default()` factory: per-platform constructor. Today every
//!     platform returns `AudioError::Unsupported` — Windows is gated on the
//!     cpal-WASAPI-loopback kill-switch smoke (phase-02 step 1); macOS / Linux
//!     are deferred per the plan.
//!
//! Why a trait now if everything is a stub? Two reasons:
//!   1. Pins the call shape so the SPA toggle UI, capability endpoint, and
//!      `desktop_audio_pipeline.rs` writer task can be wired without churning
//!      across the boundary later.
//!   2. Lets the encoder + resampler land + unit-test in isolation under the
//!      same `audio` cargo feature, so the kill-switch sprint only needs to
//!      slot a real `AudioCapture` impl into a tested pipeline.

use async_trait::async_trait;

pub mod errors;
pub mod opus_encoder;
pub mod resample;

#[cfg(target_os = "windows")]
pub mod wasapi_loopback;

#[cfg(target_os = "macos")]
pub mod sck_audio;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux_pulse;

pub use errors::AudioError;

/// Pull-based capture handle. Implementations own a backend stream (cpal,
/// Core Audio, PulseAudio, ...) and bridge it into the async runtime via an
/// internal mpsc. Frame size is left to the impl — typical: 10-20 ms at the
/// device's native rate.
#[async_trait]
pub trait AudioCapture: Send {
    /// Yield the next interleaved i16 PCM frame.
    async fn next_frame(&mut self) -> Result<Vec<i16>, AudioError>;

    /// Native sample rate of the underlying device. Caller resamples to 48
    /// kHz before Opus encode.
    fn sample_rate(&self) -> u32;

    /// Channel count (mono = 1, stereo = 2). Caller upmixes / downmixes to
    /// stereo before Opus encode.
    fn channels(&self) -> u16;
}

/// Cheap "can this binary capture audio?" probe. Used by the capabilities
/// endpoint + agent-state endpoint so the SPA renders an honest greyed-out
/// toggle without paying a device-open round-trip.
///
/// Per-platform implementations:
/// - **macOS**: `sck_audio::probe_supported` checks Screen Recording
///   permission via `SCShareableContent::get` and caches the result for
///   the binary lifetime. The audio capture itself is coupled to the
///   video SCStream (`crate::sck::SckCapture::new_with_audio`), so
///   `make_default()` returns `Unsupported` on macOS — this function is
///   the only honest signal the SPA can poll.
/// - **Windows**: `wasapi_loopback::probe_supported` enumerates the
///   default output device and reads its config (no stream construction,
///   so no WASAPI activation in audio-routing tools), caches the result
///   for the binary lifetime.
/// - **Other**: `make_default().is_ok()` — Linux is `Unsupported` today.
pub fn probe_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        sck_audio::probe_supported()
    }
    #[cfg(target_os = "windows")]
    {
        wasapi_loopback::probe_supported()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        make_default().is_ok()
    }
}

/// Construct the platform-default capture. Returns `Unsupported` on every
/// platform until phase-02's per-platform impls land. Callers MUST treat
/// `Unsupported` as a normal "audio off" outcome, not a fatal error.
pub fn make_default() -> Result<Box<dyn AudioCapture>, AudioError> {
    #[cfg(target_os = "windows")]
    {
        wasapi_loopback::make()
    }
    #[cfg(target_os = "macos")]
    {
        // macOS audio is coupled to the video SCStream — see
        // `crate::sck::SckCapture::new_with_audio`. The factory has no
        // standalone path; callers wire the SCK-side receiver directly
        // into `sck_audio::SckAudioCapture::from_receiver`. `probe_supported`
        // above is the only honest signal the SPA can poll.
        Err(AudioError::Unsupported("audio.macos.coupled-to-sck"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_pulse::make()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        Err(AudioError::Unsupported("audio.platform.unknown"))
    }
}
