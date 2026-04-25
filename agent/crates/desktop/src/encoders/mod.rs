//! H.264 encoder backends for Phase 03.
//!
//! One `H264Encoder` trait, two implementations:
//! - `VideoToolboxEncoder` — macOS hardware (primary).
//! - `OpenH264Encoder` — cross-platform software (fallback).
//!
//! All APIs here are gated behind the `h264` cargo feature so the existing
//! JPEG-only build keeps compiling unchanged during rollout.

#![cfg(feature = "h264")]

use bytes::Bytes;

#[cfg(target_os = "macos")]
pub mod videotoolbox_encoder;

pub mod openh264_encoder;

/// One encoded H.264 access unit, ready for `TrackLocalStaticSample::write_sample`.
///
/// `annexb` bytes carry start-code-delimited NAL units. On keyframes the stream
/// begins with SPS then PPS then the IDR slice — matches what browsers and the
/// webrtc-rs `H264Payloader` expect.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub annexb: Bytes,
    pub is_keyframe: bool,
    /// Monotonic presentation timestamp in microseconds from session start.
    pub pts_us: u64,
}

/// Parameter sets extracted on the first IDR so the client can build its
/// WebCodecs `VideoDecoder.configure({ description })` blob via `build_avcc`.
#[derive(Debug, Clone)]
pub struct ParameterSets {
    pub sps: Bytes,
    pub pps: Bytes,
}

/// Bitrate targets by quality tier, in bits per second.
///
/// Tuned for **screen content** (mostly UI + text), not natural video. Screen
/// captures push H.264 outside its sweet spot: Constrained Baseline + CAVLC
/// (no B-frames, no CABAC) costs ~25 % more bits than Main+CABAC for the same
/// visual quality, and text edges are unforgiving — sub-3 bpp on a 1 MP frame
/// produces visible macroblocking around glyphs and panel borders.
///
/// Reference points (per industry remote-desktop tools at ~1 MP):
/// - Parsec "Conservative" preset: ~8 Mbps
/// - Moonlight default 1080p60: ~10–15 Mbps
/// - AnyDesk Med tier: ~6–8 Mbps
///
/// Picked so Med stays comfortable on broadband and High targets LAN/wifi-5
/// without hitting the 10 Mbps ceiling that REMB defaults imply.
#[derive(Debug, Clone, Copy)]
pub struct BitrateBps(pub u32);

impl BitrateBps {
    pub const LOW: Self = Self(2_500_000);
    pub const MED: Self = Self(6_000_000);
    pub const HIGH: Self = Self(12_000_000);
}

/// Trait implemented by all H.264 backends.
///
/// Impls own their own frame counter and rate-control state. Input is always
/// BGRA (xcap's native format); each impl handles its own colour conversion
/// (VT does BGRA→NV12 on the GPU for free; OpenH264 goes through yuvutils-rs).
pub trait H264Encoder: Send {
    /// Feed one captured frame. Returns the encoded access unit if the encoder
    /// produced output for this input, `None` if it dropped the frame (e.g.
    /// OpenH264 frame-skip on bandwidth overshoot).
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>>;

    /// Update the target bitrate mid-stream. Both VT and OpenH264 support this
    /// without teardown.
    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()>;

    /// Returns the SPS/PPS parameter sets once the first IDR has been encoded.
    /// Before that, returns `None`. Returns an owned clone because the VT
    /// backend keeps params behind a `Mutex` (populated from a C callback
    /// thread) and can't hand out borrows. Callers invoke this only on the
    /// first keyframe so the clone cost (two `Bytes` Arc-clones) is trivial.
    fn parameter_sets(&self) -> Option<ParameterSets>;
}
