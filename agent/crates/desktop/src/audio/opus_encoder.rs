//! Opus encoder wrapper (phase-02 scaffold).
//!
//! Wire format pinned by phase-02 spec:
//!   - 48 kHz sample rate
//!   - stereo (2 channels)
//!   - 20 ms frames → 960 samples per channel → 1920 interleaved i16 per frame
//!   - VBR ~96 kbps, `Application::Audio` (general-purpose, balanced for music
//!     + speech; `RestrictedLowDelay` is overkill — Opus's normal delay is ~26 ms)
//!
//! Caller responsibility: deliver exactly 1920-sample interleaved stereo i16
//! frames (resampled upstream by `super::resample`). The encoder itself does
//! NO buffering — short or long frames return `EncodeFailed`.

use super::AudioError;

/// Opus payload size cap. webrtc-rs RTP payloads happily fit in <1500 B but
/// we size for libopus's max recommended ceiling (4 KB) to avoid clipping
/// rare DTX / silence-padded frames.
pub const MAX_OPUS_PAYLOAD: usize = 4000;

/// Frame timing: 20 ms at 48 kHz = 960 samples per channel.
pub const SAMPLES_PER_CHANNEL: usize = 960;

/// Stereo: 1920 interleaved i16 samples per frame.
pub const FRAME_SAMPLES_STEREO: usize = SAMPLES_PER_CHANNEL * 2;

/// VBR target bitrate matched to Parsec / Jump audio quality.
pub const TARGET_BITRATE_BPS: i32 = 96_000;

/// Encoder state. Wrapped so the `audio` feature flag is the only knob the
/// rest of the crate has to think about.
pub struct OpusEncoder {
    #[cfg(feature = "audio")]
    inner: audiopus::coder::Encoder,
}

impl OpusEncoder {
    /// Construct with the phase-02 wire format (48 kHz / stereo / 96 kbps VBR).
    pub fn new() -> Result<Self, AudioError> {
        #[cfg(feature = "audio")]
        {
            use audiopus::{Application, Bitrate, Channels, SampleRate};
            let mut enc = audiopus::coder::Encoder::new(
                SampleRate::Hz48000,
                Channels::Stereo,
                Application::Audio,
            )
            .map_err(|e| AudioError::EncodeFailed(format!("encoder init: {e}")))?;
            enc.set_bitrate(Bitrate::BitsPerSecond(TARGET_BITRATE_BPS))
                .map_err(|e| AudioError::EncodeFailed(format!("set_bitrate: {e}")))?;
            // VBR is the default but pin it explicitly so config drift can't
            // silently flip us into CBR (which inflates bitrate on silence).
            enc.set_vbr(true)
                .map_err(|e| AudioError::EncodeFailed(format!("set_vbr: {e}")))?;
            Ok(Self { inner: enc })
        }
        #[cfg(not(feature = "audio"))]
        {
            Err(AudioError::Unsupported("audio.feature.disabled"))
        }
    }

    /// Encode one 20 ms frame (1920 interleaved i16 stereo samples). Returns
    /// the Opus-compressed bytes (writes into a fresh `Vec` sized to the
    /// libopus output length).
    pub fn encode_frame(&mut self, pcm: &[i16]) -> Result<Vec<u8>, AudioError> {
        if pcm.len() != FRAME_SAMPLES_STEREO {
            return Err(AudioError::EncodeFailed(format!(
                "expected {FRAME_SAMPLES_STEREO} samples, got {}",
                pcm.len()
            )));
        }
        #[cfg(feature = "audio")]
        {
            let mut buf = vec![0u8; MAX_OPUS_PAYLOAD];
            let written = self
                .inner
                .encode(pcm, &mut buf)
                .map_err(|e| AudioError::EncodeFailed(format!("encode: {e}")))?;
            buf.truncate(written);
            Ok(buf)
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = pcm;
            Err(AudioError::Unsupported("audio.feature.disabled"))
        }
    }
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;

    #[test]
    fn encodes_silence_to_short_frame() {
        // VBR Opus on pure silence with `Application::Audio` produces a small
        // but non-zero payload (a few bytes per frame). Bound it generously.
        let mut enc = OpusEncoder::new().expect("encoder init");
        let pcm = vec![0i16; FRAME_SAMPLES_STEREO];
        let payload = enc.encode_frame(&pcm).expect("encode");
        assert!(!payload.is_empty(), "silence frame should not be empty");
        assert!(
            payload.len() < MAX_OPUS_PAYLOAD,
            "silence frame should be much smaller than the cap"
        );
    }

    #[test]
    fn rejects_wrong_frame_size() {
        let mut enc = OpusEncoder::new().expect("encoder init");
        let too_small = vec![0i16; FRAME_SAMPLES_STEREO - 2];
        assert!(matches!(
            enc.encode_frame(&too_small),
            Err(AudioError::EncodeFailed(_))
        ));
    }
}
