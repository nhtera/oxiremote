//! H.264 and HEVC encoder backends.
//!
//! `H264Encoder` trait — two implementations:
//! - `VideoToolboxEncoder` — macOS hardware (primary).
//! - `OpenH264Encoder` — cross-platform software (fallback).
//!
//! `HevcEncoder` trait — one implementation (macOS VT only, Phase 01+).
//!
//! All H.264 APIs are gated behind the `h264` cargo feature; HEVC behind
//! `hevc`. The JPEG-only build compiles with neither.

// Shared VT helpers (pixel buffer wrap, property set, force-IDR dict).
// Available when either h264 or hevc is enabled on macOS.
#[cfg(all(any(feature = "h264", feature = "hevc"), target_os = "macos"))]
pub(crate) mod vt_common;

#[cfg(feature = "h264")]
#[cfg(target_os = "macos")]
pub mod videotoolbox_encoder;

#[cfg(feature = "h264")]
pub mod openh264_encoder;

#[cfg(feature = "hevc")]
#[cfg(target_os = "macos")]
pub mod videotoolbox_hevc_encoder;

// ─── Shared frame type ───────────────────────────────────────────────────────

/// One encoded access unit, ready for `TrackLocalStaticSample::write_sample`.
///
/// `annexb` bytes carry start-code-delimited NAL units. On keyframes the
/// stream begins with parameter sets then the IDR slice.
#[cfg(any(feature = "h264", feature = "hevc"))]
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub annexb: bytes::Bytes,
    pub is_keyframe: bool,
    /// Monotonic presentation timestamp in microseconds from session start.
    pub pts_us: u64,
}

// ─── Bitrate presets ─────────────────────────────────────────────────────────

/// Bitrate targets by quality tier, in bits per second.
///
/// Tuned for **screen content** (mostly UI + text), not natural video. As of
/// the 2026-05-13 quality uplift (`260513-0009-h264-quality-uplift-vt-high-profile`)
/// the VideoToolbox encoder uses **H.264 High profile + CABAC**, so these
/// presets now buy noticeably sharper text per Mbps than phase-01's
/// Baseline + CAVLC configuration. HiDPI sessions double each preset and
/// clamp at **30 Mbps** (raised from 20 Mbps) — see `tier_bitrate` in
/// `desktop_ws.rs`.
///
/// Reference points (per industry remote-desktop tools at ~1 MP):
/// - Parsec "Conservative" preset: ~8 Mbps
/// - Moonlight default 1080p60: ~10–15 Mbps
/// - Sunshine NVENC 1440p60: ~30 Mbps (matches our HiDPI cap)
/// - AnyDesk Med tier: ~6–8 Mbps
///
/// Picked so Med stays comfortable on broadband and High targets LAN/wifi-5
/// without hitting the 10 Mbps ceiling that REMB defaults imply.
#[cfg(any(feature = "h264", feature = "hevc"))]
#[derive(Debug, Clone, Copy)]
pub struct BitrateBps(pub u32);

#[cfg(any(feature = "h264", feature = "hevc"))]
impl BitrateBps {
    pub const LOW: Self = Self(2_500_000);
    pub const MED: Self = Self(6_000_000);
    pub const HIGH: Self = Self(12_000_000);
}

// ─── H.264 types + trait ─────────────────────────────────────────────────────

/// Parameter sets extracted on the first IDR so the client can build its
/// WebCodecs `VideoDecoder.configure({ description })` blob via `build_avcc`.
///
/// `is_hardware` rides on the same first-IDR oneshot so the session layer
/// can emit `SignalOut::Pipeline { hardware_accel }` to the SPA pill without
/// adding a second channel.
#[cfg(feature = "h264")]
#[derive(Debug, Clone)]
pub struct ParameterSets {
    pub sps: bytes::Bytes,
    pub pps: bytes::Bytes,
    pub is_hardware: bool,
}

/// Trait implemented by all H.264 backends.
///
/// Impls own their own frame counter and rate-control state. Input is always
/// BGRA (xcap's native format); each impl handles its own colour conversion
/// (VT does BGRA→NV12 on the GPU for free; OpenH264 goes through yuvutils-rs).
#[cfg(feature = "h264")]
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
    /// thread) and can't hand out borrows.
    fn parameter_sets(&self) -> Option<ParameterSets>;

    /// True when the backend offloads encode work to dedicated silicon.
    /// Software backends (OpenH264) return false.
    fn is_hardware(&self) -> bool;
}

// ─── HEVC types + trait ──────────────────────────────────────────────────────

/// VPS + SPS + PPS NAL units extracted on the first HEVC IDR.
///
/// Used by the hvcC builder (Phase 04) to assemble the WebCodecs
/// `VideoDecoder.configure({ description })` blob.
#[cfg(feature = "hevc")]
#[derive(Debug, Clone)]
pub struct HevcParameterSets {
    /// Video Parameter Set — NAL type 32.
    pub vps: bytes::Bytes,
    /// Sequence Parameter Set — NAL type 33.
    pub sps: bytes::Bytes,
    /// Picture Parameter Set — NAL type 34.
    pub pps: bytes::Bytes,
    pub is_hardware: bool,
}

/// Trait implemented by HEVC encoder backends.
#[cfg(feature = "hevc")]
pub trait HevcEncoder: Send {
    /// Feed one captured frame; returns the encoded access unit or `None`.
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>>;

    /// Update the target bitrate mid-stream.
    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()>;

    /// Returns VPS+SPS+PPS once the first IDR has been encoded; `None` before.
    fn parameter_sets(&self) -> Option<HevcParameterSets>;

    /// True when the backend uses dedicated encode silicon.
    fn is_hardware(&self) -> bool;
}
