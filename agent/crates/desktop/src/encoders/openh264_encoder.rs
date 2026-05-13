//! OpenH264 software encoder — cross-platform fallback for non-macOS builds
//! and for macOS hosts where VideoToolbox fails to initialise.
//!
//! Research memo `researcher-260425-0050-h264-encoder-bindings.md` locks the
//! encoder crate (`openh264 0.9`, `source` feature) and the BGRA→I420 path
//! (`yuvutils-rs 0.8`, NEON on Apple Silicon / AVX2 on x86).
//!
//! The memo's recommended method names needed two corrections verified against
//! the on-disk crate source:
//! - `EncoderConfig::bitrate(BitRate::from_bps(n))` not `set_bitrate_bps(n)`
//! - `EncoderConfig::skip_frames(true)` not `enable_skip_frame(true)`
//!
//! The high-level `openh264` crate does **not** expose a runtime bitrate
//! setter — the underlying ISVCEncoder SetOption is wrapped but `raw_api`
//! is private. `set_bitrate` therefore tears down and recreates the encoder;
//! the next `encode()` call emits a fresh IDR naturally, so the caller doesn't
//! need to `force_idr`.

#![cfg(feature = "h264")]

use bytes::Bytes;
use openh264::{
    OpenH264API,
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, Level, Profile},
    formats::YUVSource,
};
use tracing::debug;
use yuvutils_rs::{
    BufferStoreMut, YuvChromaSubsampling, YuvConversionMode, YuvPlanarImageMut, YuvRange,
    YuvStandardMatrix, bgra_to_yuv420,
};

use super::{BitrateBps, EncodedFrame, H264Encoder, ParameterSets};
use crate::h264_format::{NAL_TYPE_IDR, NAL_TYPE_PPS, NAL_TYPE_SPS, nal_unit_type, split_annexb};

const DEFAULT_FRAME_DURATION_US: u64 = 16_666;

pub struct OpenH264Encoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    bitrate: BitrateBps,
    params: Option<ParameterSets>,
    frame_counter: u64,
}

// `openh264::encoder::Encoder` declares `unsafe impl Send + Sync` itself at
// src/encoder.rs:832-833, so we inherit it.

fn build_config(bitrate: BitrateBps) -> EncoderConfig {
    EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate.0))
        // Frame skip required for OpenH264 to honour the bitrate target —
        // without it the encoder can overshoot on scene cuts (Cisco docs).
        .skip_frames(true)
        // Hint — 60 FPS cap; the encoder also accepts lower per-frame rates.
        .max_frame_rate(FrameRate::from_hz(60.0))
        // Match the SDP fmtp `profile-level-id=640032` advertised by
        // `h264_codec_capability()`. OpenH264's default is Constrained
        // Baseline (profile_idc=66), but the SDP claims High (profile_idc=100)
        // since the 2026-05-13 VT quality uplift. Without this override the
        // SPS NAL units emitted on Windows/Linux disagree with the negotiated
        // profile — Chrome's D3D11VA hardware decoder (configured per the
        // SDP) then renders Baseline streams as black frames.
        //
        // OpenH264's "High" profile is CAVLC-only (no CABAC / 8×8 transform);
        // VT's High profile uses CABAC. Both emit profile_idc=100 SPS so the
        // browser decoder is happy. Level 5.0 matches `level_idc=0x32` in
        // the fmtp.
        .profile(Profile::High)
        .level(Level::Level_5_0)
}

impl OpenH264Encoder {
    pub fn new(width: u32, height: u32, initial_bitrate: BitrateBps) -> anyhow::Result<Self> {
        anyhow::ensure!(width > 0 && height > 0, "invalid dims {}x{}", width, height);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), build_config(initial_bitrate))
            .map_err(|e| anyhow::anyhow!("OpenH264 Encoder::with_api_config: {e:?}"))?;
        Ok(Self {
            encoder,
            width,
            height,
            bitrate: initial_bitrate,
            params: None,
            frame_counter: 0,
        })
    }
}

/// Minimal YUVSource view over a `YuvPlanarImageMut<u8>` buffer — the openh264
/// encoder reads dimensions, strides and the three planes by reference.
struct YuvView<'a> {
    image: &'a YuvPlanarImageMut<'a, u8>,
}

impl YUVSource for YuvView<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.image.width as usize, self.image.height as usize)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (
            self.image.y_stride as usize,
            self.image.u_stride as usize,
            self.image.v_stride as usize,
        )
    }
    fn y(&self) -> &[u8] {
        borrow(&self.image.y_plane)
    }
    fn u(&self) -> &[u8] {
        borrow(&self.image.u_plane)
    }
    fn v(&self) -> &[u8] {
        borrow(&self.image.v_plane)
    }
}

fn borrow<'a>(store: &'a BufferStoreMut<'a, u8>) -> &'a [u8] {
    store.borrow()
}

/// Scan an Annex-B bitstream for SPS + PPS NALs and return them as a
/// `ParameterSets` pair if both are present. Used to cache the avcC source on
/// the first IDR.
fn extract_sps_pps(annexb: &[u8]) -> Option<ParameterSets> {
    let mut sps: Option<&[u8]> = None;
    let mut pps: Option<&[u8]> = None;
    for nal in split_annexb(annexb) {
        match nal_unit_type(nal) {
            Some(NAL_TYPE_SPS) => sps.get_or_insert(nal),
            Some(NAL_TYPE_PPS) => pps.get_or_insert(nal),
            _ => continue,
        };
    }
    match (sps, pps) {
        (Some(s), Some(p)) => Some(ParameterSets {
            sps: Bytes::copy_from_slice(s),
            pps: Bytes::copy_from_slice(p),
            // OpenH264 is software-only; the trait method is the source of
            // truth but extract_sps_pps is a free function called outside an
            // impl block — locking the value here avoids threading the
            // encoder ref through. Keep in sync with H264Encoder::is_hardware.
            is_hardware: false,
        }),
        _ => None,
    }
}

/// Detect a keyframe by scanning for an IDR NAL. OpenH264's default
/// sps_pps_strategy inlines SPS+PPS before each IDR, so presence of IDR is
/// the authoritative signal.
fn contains_idr(annexb: &[u8]) -> bool {
    split_annexb(annexb)
        .iter()
        .any(|n| nal_unit_type(n) == Some(NAL_TYPE_IDR))
}

impl H264Encoder for OpenH264Encoder {
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        anyhow::ensure!(
            width == self.width && height == self.height,
            "OpenH264Encoder dim mismatch: configured {}x{}, got {}x{}",
            self.width,
            self.height,
            width,
            height
        );
        let expected_len = (width as usize) * (height as usize) * 4;
        anyhow::ensure!(
            bgra.len() >= expected_len,
            "BGRA buffer too short: got {} bytes, need {}",
            bgra.len(),
            expected_len
        );

        if force_idr {
            self.encoder.force_intra_frame();
        }

        // Allocate a fresh YUV420 target each frame. Future optimisation: pool
        // reuse via a cached YuvPlanarImageMut on the encoder struct. For now
        // allocation cost is ~width*height*1.5 bytes (~3 MB at 1080p).
        let mut yuv = YuvPlanarImageMut::<u8>::alloc(width, height, YuvChromaSubsampling::Yuv420);
        let bgra_stride = width * 4;
        bgra_to_yuv420(
            &mut yuv,
            &bgra[..expected_len],
            bgra_stride,
            YuvRange::Limited,
            YuvStandardMatrix::Bt709,
            YuvConversionMode::Balanced,
        )
        .map_err(|e| anyhow::anyhow!("bgra_to_yuv420: {e:?}"))?;

        let bitstream = self
            .encoder
            .encode(&YuvView { image: &yuv })
            .map_err(|e| anyhow::anyhow!("openh264 encode: {e:?}"))?;
        let annexb = bitstream.to_vec();
        if annexb.is_empty() {
            // Encoder skipped this frame (bitrate control decision).
            debug!("OpenH264 skipped frame");
            return Ok(None);
        }

        let is_keyframe = contains_idr(&annexb);
        if is_keyframe && self.params.is_none() {
            self.params = extract_sps_pps(&annexb);
        }

        self.frame_counter = self.frame_counter.wrapping_add(1);
        let pts_us = self.frame_counter.saturating_mul(DEFAULT_FRAME_DURATION_US);

        Ok(Some(EncodedFrame {
            annexb: Bytes::from(annexb),
            is_keyframe,
            pts_us,
        }))
    }

    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()> {
        // High-level openh264 crate doesn't expose ISVCEncoder::SetOption —
        // recreate. Next encode naturally emits an IDR so the decoder resyncs.
        self.encoder =
            Encoder::with_api_config(OpenH264API::from_source(), build_config(bitrate))
                .map_err(|e| anyhow::anyhow!("OpenH264 recreate on set_bitrate: {e:?}"))?;
        self.bitrate = bitrate;
        // Reset cached params — the new encoder will emit fresh SPS/PPS on
        // its next IDR.
        self.params = None;
        Ok(())
    }

    fn parameter_sets(&self) -> Option<ParameterSets> {
        self.params.clone()
    }

    fn is_hardware(&self) -> bool {
        // OpenH264 is pure CPU.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sps_pps_finds_both_from_annexb() {
        let sps: &[u8] = &[0x67, 0x42, 0x00, 0x1F, 0xAA];
        let pps: &[u8] = &[0x68, 0xCE, 0x38, 0x80];
        let idr: &[u8] = &[0x65, 0x11, 0x22];
        let mut stream = Vec::new();
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        stream.extend_from_slice(sps);
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        stream.extend_from_slice(pps);
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        stream.extend_from_slice(idr);
        let ps = extract_sps_pps(&stream).expect("must find both");
        assert_eq!(&ps.sps[..], sps);
        assert_eq!(&ps.pps[..], pps);
    }

    #[test]
    fn extract_sps_pps_returns_none_when_missing() {
        let idr_only: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x11];
        assert!(extract_sps_pps(idr_only).is_none());
    }

    #[test]
    fn contains_idr_detects_type_5() {
        let mut s = Vec::new();
        s.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x01]); // IDR
        assert!(contains_idr(&s));

        let mut t = Vec::new();
        t.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x61, 0x01]); // P-slice
        assert!(!contains_idr(&t));
    }

    /// End-to-end smoke: feed a synthetic BGRA frame through the real Cisco
    /// OpenH264 encoder built into the binary via the `source` feature,
    /// verify we get an IDR with SPS+PPS extracted. Validates the entire
    /// BGRA→I420→H264→Annex-B chain without needing macOS screen-recording
    /// permission.
    #[test]
    fn encoder_emits_idr_on_first_frame() {
        let width = 64u32;
        let height = 64u32;
        let mut enc = OpenH264Encoder::new(width, height, BitrateBps::LOW)
            .expect("encoder construction must succeed");

        // Solid grey BGRA frame. Content is irrelevant — we only assert
        // that an IDR is emitted and SPS/PPS are discovered.
        let bgra = vec![128u8; (width * height * 4) as usize];
        let frame = enc
            .encode(&bgra, width, height, false)
            .expect("encode must not error")
            .expect("first frame must not be skipped");

        assert!(frame.is_keyframe, "first frame must be keyframe");
        assert!(!frame.annexb.is_empty(), "non-empty Annex-B");
        let params = enc
            .parameter_sets()
            .expect("SPS/PPS must be cached after first IDR");
        assert!(!params.sps.is_empty(), "non-empty SPS");
        assert!(!params.pps.is_empty(), "non-empty PPS");
        // SPS first byte is the NAL header; low 5 bits = 7.
        assert_eq!(params.sps[0] & 0x1F, NAL_TYPE_SPS);
        assert_eq!(params.pps[0] & 0x1F, NAL_TYPE_PPS);
    }

    /// Regression guard for the Windows / Linux H.264 black-screen bug
    /// fixed alongside the 2026-05-13 VT quality uplift (commit 7fed601):
    /// the SDP fmtp advertises `profile-level-id=640032` (High 5.0); without
    /// `build_config()` setting `Profile::High` the encoder defaulted to
    /// Constrained Baseline (profile_idc=66) and browsers' D3D11VA decoders
    /// rendered the stream as black frames. The SPS profile_idc lives at
    /// byte index 1 (byte 0 is the NAL header); 0x64 = 100 = High profile.
    #[test]
    fn sps_advertises_high_profile() {
        let width = 64u32;
        let height = 64u32;
        let mut enc =
            OpenH264Encoder::new(width, height, BitrateBps::LOW).expect("encoder");
        let bgra = vec![128u8; (width * height * 4) as usize];
        let _ = enc
            .encode(&bgra, width, height, false)
            .expect("encode must succeed")
            .expect("first frame must not be skipped");
        let params = enc.parameter_sets().expect("SPS must be cached");
        assert!(params.sps.len() >= 4, "SPS too short: {} bytes", params.sps.len());
        assert_eq!(
            params.sps[1], 0x64,
            "SPS profile_idc must be 100 (High) to match SDP profile-level-id=640032, got 0x{:02x}",
            params.sps[1]
        );
        // Level 5.0 = level_idc 0x32 lives at SPS byte 3 (after profile_idc
        // + constraint_set bits). OpenH264 may emit a lower level if the
        // resolution allows it; assert it does not exceed 5.0 so the
        // negotiated level envelope is respected.
        assert!(
            params.sps[3] <= 0x32,
            "SPS level_idc must be ≤ 0x32 (Level 5.0), got 0x{:02x}",
            params.sps[3]
        );
    }

    #[test]
    fn force_idr_after_non_keyframe() {
        let width = 64u32;
        let height = 64u32;
        let mut enc =
            OpenH264Encoder::new(width, height, BitrateBps::LOW).expect("encoder");

        let bgra = vec![128u8; (width * height * 4) as usize];
        let _first = enc.encode(&bgra, width, height, false).unwrap().unwrap();
        // Second frame: same content → may be delta (may even be skipped,
        // but with skip_frames enabled at low bitrate solid-grey → tiny
        // delta → usually emitted). Don't assert on keyframe flag here.
        let _second = enc.encode(&bgra, width, height, false).unwrap();
        // Force IDR on the third frame.
        let third = enc
            .encode(&bgra, width, height, true)
            .unwrap()
            .expect("force-idr frame must not be skipped");
        assert!(third.is_keyframe, "force_idr must produce keyframe");
    }
}
