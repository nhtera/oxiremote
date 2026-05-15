//! H.264, VP9, and AV1 encoder backends.
//!
//! `H264Encoder` trait — two implementations:
//! - `VideoToolboxEncoder` — macOS hardware (primary).
//! - `OpenH264Encoder` — cross-platform software (fallback).
//!
//! `Vp9Encoder` trait — one implementation (libvpx software, screen-content
//! tuned to match Chrome Remote Desktop's `webrtc_video_encoder_vpx.cc`).
//!
//! `Av1Encoder` trait — one implementation (libaom software, screen-content
//! tuned via `AOM_CONTENT_SCREEN` + IBC + palette).
//!
//! All H.264 APIs are gated behind the `h264` cargo feature; VP9 behind `vp9`;
//! AV1 behind `av1`. The JPEG-only build compiles with none.

// Shared VT helpers (pixel buffer wrap, property set, force-IDR dict).
// Available on macOS when h264 is enabled.
#[cfg(all(feature = "h264", target_os = "macos"))]
pub(crate) mod vt_common;

#[cfg(feature = "h264")]
#[cfg(target_os = "macos")]
pub mod videotoolbox_encoder;

#[cfg(feature = "h264")]
pub mod openh264_encoder;

// Phase-02: VP9 via libvpx (cross-platform software, screen-content tuned).
#[cfg(feature = "vp9")]
pub mod vpx_sys;
#[cfg(feature = "vp9")]
pub mod vp9_encoder;

// Phase-05: AV1 via libaom (cross-platform software, AOM_CONTENT_SCREEN
// with IBC + palette — only software AV1 path with screen-content tools).
#[cfg(feature = "av1")]
pub mod aom_sys;
#[cfg(feature = "av1")]
pub mod av1_encoder;

// ─── Shared frame type ───────────────────────────────────────────────────────

/// One encoded access unit, ready for `TrackLocalStaticSample::write_sample`.
///
/// `annexb` bytes carry start-code-delimited NAL units. On keyframes the
/// stream begins with parameter sets then the IDR slice.
#[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Raw bitstream bytes ready for `TrackLocalStaticSample::write_sample`.
    /// For H.264 this is Annex-B (start-code-delimited NAL units). For VP9
    /// this is the IVF-payload-equivalent raw VP9 frame from
    /// `vpx_codec_get_cx_data`. For AV1 this is the OBU sequence from
    /// `aom_codec_get_cx_data`; our custom Av1Payloader handles OBU
    /// fragmentation per the AV1 RTP spec (RFC 9606 / v1.0).
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
#[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
#[derive(Debug, Clone, Copy)]
pub struct BitrateBps(pub u32);

#[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
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

// ─── VP9 types + trait ───────────────────────────────────────────────────────

/// VP9 frame output is self-contained — no out-of-band parameter sets
/// (no avcC equivalent). The first keyframe carries the sequence header
/// inline. This type is kept around as a parallel to `ParameterSets` so
/// the pipeline runner can surface `is_hardware` to the session layer.
#[cfg(feature = "vp9")]
#[derive(Debug, Clone, Copy)]
pub struct Vp9SequenceInfo {
    /// libvpx is always software. Reserved for the day a hardware VP9
    /// encoder backend lands (e.g. macOS VT VP9 on M3+).
    pub is_hardware: bool,
}

/// Trait implemented by VP9 encoder backends. Input is BGRA (xcap's native
/// format); implementations handle BGRA→I420 via `yuvutils-rs`.
///
/// Configuration matches Chrome Remote Desktop's `webrtc_video_encoder_vpx.cc`:
/// `cpu-used=6`, `VP9E_CONTENT_SCREEN`, `AQ_MODE=3` (cyclic refresh),
/// `ROW_MT=1`, `TILE_COLUMNS=log2(threads)`, CBR end-usage, `kf_max_dist=10000`
/// (externally triggered keyframes only), `lag_in_frames=0` (no lookahead).
#[cfg(feature = "vp9")]
pub trait Vp9Encoder: Send {
    /// Phase-03b: hint to the encoder which 16×16 macroblocks need
    /// re-encoding via `VP9E_SET_ACTIVE_MAP`. Caller passes the dirty
    /// rects from SCK / DXGI for this frame; the encoder maps them to
    /// macroblock granularity and skips untouched blocks.
    ///
    /// `force_full=true` (used on keyframes / cold-start) marks every
    /// block as active, ignoring the dirty rects. Empty `rects` with
    /// `force_full=false` also means "no info — assume full" so the
    /// encoder never produces black blocks from an incorrect skip map.
    fn apply_dirty_rects(
        &mut self,
        _frame_width: u32,
        _frame_height: u32,
        _rects: &[crate::capture::DirtyRect],
        _force_full: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Feed one captured BGRA frame. Returns the encoded VP9 frame, or
    /// `None` if libvpx dropped output (e.g. all 16×16 blocks skipped via
    /// active-map in phase-03).
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>>;

    /// Update the CBR target bitrate mid-stream. Drives `rc_target_bitrate`
    /// via `vpx_codec_enc_config_set`. No encoder teardown.
    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()>;

    /// Returns `Some` once init succeeded. Carries `is_hardware = false`
    /// for libvpx; kept as an `Option` for trait parity with H.264.
    fn sequence_info(&self) -> Option<Vp9SequenceInfo>;

    /// libvpx is software-only — always returns `false`. Trait method exists
    /// so the pipeline runner can mirror the `H264Encoder` shape and feed
    /// `SignalOut::Pipeline { hardware_accel }`.
    fn is_hardware(&self) -> bool;
}

// ─── AV1 types + trait ───────────────────────────────────────────────────────

/// AV1 sequence info surfaced once the first keyframe encodes. AV1 has no
/// out-of-band parameter set (the sequence header is the first OBU in the
/// keyframe and is carried inline through the RTP payloader), so this type
/// only carries `is_hardware` — kept for trait symmetry with H.264.
#[cfg(feature = "av1")]
#[derive(Debug, Clone, Copy)]
pub struct Av1SequenceInfo {
    /// libaom is always software. Reserved for future hardware AV1 backends
    /// (macOS VT AV1 on M3+, NVENC AV1 on RTX 40+).
    pub is_hardware: bool,
}

/// Trait implemented by AV1 encoder backends. Configuration matches Chrome
/// Remote Desktop's `remoting/codec/webrtc_video_encoder_av1.cc`:
/// `cpu_used=11` (RT mode), `AV1E_SET_TUNE_CONTENT=AOM_CONTENT_SCREEN`
/// (palette + IBC), `kf_mode=AOM_KF_DISABLED`, `rc_min_quantizer=10`,
/// `rc_max_quantizer=52`, `g_lag_in_frames=0`, `rc_end_usage=AOM_CBR`,
/// superblock 64×64 when threads > 4. Tile columns + rows = log2(threads).
#[cfg(feature = "av1")]
pub trait Av1Encoder: Send {
    /// Phase-03b: dirty-rect → active-map (`AOME_SET_ACTIVEMAP`). Same
    /// contract as `Vp9Encoder::apply_dirty_rects`.
    fn apply_dirty_rects(
        &mut self,
        _frame_width: u32,
        _frame_height: u32,
        _rects: &[crate::capture::DirtyRect],
        _force_full: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Feed one BGRA frame. Returns the encoded AV1 OBU sequence, or `None`
    /// if libaom dropped output.
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>>;

    /// Update the CBR target bitrate mid-stream.
    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()>;

    fn sequence_info(&self) -> Option<Av1SequenceInfo>;
    fn is_hardware(&self) -> bool;
}
