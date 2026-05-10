//! i16 → f32 → resample → i16 wrapper around `rubato` (phase-02 scaffold).
//!
//! Capture devices land at their native rate (44.1 / 48 / 96 kHz typical on
//! Windows). The Opus wire format is fixed at 48 kHz stereo, so anything off
//! 48 kHz needs resampling before `super::opus_encoder::encode_frame`.
//!
//! Channel handling is upstream's job (mono → stereo upmix happens in the
//! capture impl). This module strictly does sample-rate conversion on stereo
//! frames.

use super::AudioError;

/// Wire-format target for Opus.
pub const TARGET_SAMPLE_RATE: u32 = 48_000;

/// Stateful resampler. Holds rubato's internal sinc state across calls so
/// frame-by-frame resampling stays continuous (no boundary artifacts).
pub struct Resampler {
    src_rate: u32,
    #[cfg(feature = "audio")]
    inner: rubato::FastFixedIn<f32>,
    /// Reusable f32 scratch (one channel per inner Vec). Avoids reallocating
    /// per frame on the audio hot path.
    #[cfg(feature = "audio")]
    scratch_in: Vec<Vec<f32>>,
}

impl Resampler {
    /// Construct a fixed-input resampler from `src_rate` → 48 kHz stereo.
    /// `chunk_samples_per_channel` is the per-call input size (e.g. 882 for
    /// 44.1 kHz @ 20 ms; rubato handles the fractional-output rounding).
    pub fn new(src_rate: u32, chunk_samples_per_channel: usize) -> Result<Self, AudioError> {
        if src_rate == 0 {
            return Err(AudioError::ResampleFailed("src rate is zero".into()));
        }
        #[cfg(feature = "audio")]
        {
            use rubato::{FastFixedIn, PolynomialDegree};
            let ratio = TARGET_SAMPLE_RATE as f64 / src_rate as f64;
            let inner = FastFixedIn::<f32>::new(
                ratio,
                1.0, // max_resample_ratio_relative — no async ratio adjust
                PolynomialDegree::Septic,
                chunk_samples_per_channel,
                2, // stereo
            )
            .map_err(|e| AudioError::ResampleFailed(format!("rubato init: {e}")))?;
            Ok(Self {
                src_rate,
                inner,
                scratch_in: vec![
                    vec![0.0; chunk_samples_per_channel],
                    vec![0.0; chunk_samples_per_channel],
                ],
            })
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = chunk_samples_per_channel;
            Ok(Self { src_rate })
        }
    }

    pub fn src_rate(&self) -> u32 {
        self.src_rate
    }

    /// Resample one chunk of interleaved stereo i16 to interleaved stereo i16
    /// at 48 kHz. If `src_rate == 48_000`, returns the input verbatim (no
    /// allocation beyond the output Vec).
    pub fn process_interleaved(&mut self, input: &[i16]) -> Result<Vec<i16>, AudioError> {
        if !input.len().is_multiple_of(2) {
            return Err(AudioError::ResampleFailed(format!(
                "interleaved stereo input must be even length, got {}",
                input.len()
            )));
        }
        if self.src_rate == TARGET_SAMPLE_RATE {
            return Ok(input.to_vec());
        }

        #[cfg(feature = "audio")]
        {
            use rubato::Resampler as _;
            let frames_in = input.len() / 2;
            if frames_in != self.scratch_in[0].len() {
                return Err(AudioError::ResampleFailed(format!(
                    "expected {} frames per channel, got {}",
                    self.scratch_in[0].len(),
                    frames_in,
                )));
            }
            // Deinterleave into f32 planes (rubato wants planar).
            for (i, chunk) in input.chunks_exact(2).enumerate() {
                self.scratch_in[0][i] = i16_to_f32(chunk[0]);
                self.scratch_in[1][i] = i16_to_f32(chunk[1]);
            }
            let out_planar = self
                .inner
                .process(&self.scratch_in, None)
                .map_err(|e| AudioError::ResampleFailed(format!("rubato process: {e}")))?;
            // Reinterleave + clamp back to i16.
            let frames_out = out_planar[0].len();
            let mut out = Vec::with_capacity(frames_out * 2);
            for (left, right) in out_planar[0].iter().zip(out_planar[1].iter()) {
                out.push(f32_to_i16(*left));
                out.push(f32_to_i16(*right));
            }
            Ok(out)
        }
        #[cfg(not(feature = "audio"))]
        {
            Err(AudioError::Unsupported("audio.feature.disabled"))
        }
    }
}

#[cfg(feature = "audio")]
#[inline]
fn i16_to_f32(s: i16) -> f32 {
    s as f32 / i16::MAX as f32
}

#[cfg(feature = "audio")]
#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rate_matches() {
        let mut r = Resampler::new(48_000, 960).expect("init");
        let pcm: Vec<i16> = (0..1920).map(|i| (i % 1000) as i16).collect();
        let out = r.process_interleaved(&pcm).expect("process");
        assert_eq!(out, pcm, "48k input should pass through unchanged");
    }

    #[test]
    fn resamples_44k_to_48k_with_expected_length() {
        // 44.1k → 48k with 882 samples-per-channel input @ 20 ms ≈ 960 out
        // (allow rubato's polynomial slack of ±a few samples).
        let mut r = Resampler::new(44_100, 882).expect("init");
        let input = vec![0i16; 882 * 2];
        let out = r.process_interleaved(&input).expect("process");
        let frames_out = out.len() / 2;
        assert!(
            (950..=970).contains(&frames_out),
            "expected ~960 stereo frames out, got {frames_out}"
        );
    }

    #[test]
    fn rejects_odd_interleaved_length() {
        let mut r = Resampler::new(44_100, 882).expect("init");
        assert!(matches!(
            r.process_interleaved(&[0i16; 7]),
            Err(AudioError::ResampleFailed(_))
        ));
    }
}
