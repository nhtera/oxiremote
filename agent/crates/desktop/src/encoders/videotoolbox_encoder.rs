//! VideoToolbox H.264 encoder — macOS hardware path.
//!
//! Profile: **High (`profile_idc=100`)** + **CABAC** entropy + AutoLevel.
//! Bumped from phase-01's Constrained Baseline + CAVLC on 2026-05-13 (plan
//! `260513-0009-h264-quality-uplift-vt-high-profile`). High enables 8×8
//! adaptive transform + CABAC, both critical for sharp text on screen
//! content. All target browsers (iPad Safari 17+, Chrome desktop/Android,
//! Firefox 130+) hardware-decode H.264 High 4:2:0 over WebRTC in 2026.
//! The matching SDP `profile-level-id=640032` lives in
//! `agent/src/video_pipeline.rs`.
//!
//! Reference lines (in `~/.cargo/registry/src/…`):
//! - `objc2-video-toolbox-0.3.2/.../VTCompressionSession.rs:131` create
//! - `objc2-video-toolbox-0.3.2/.../VTCompressionSession.rs:293` encode_frame
//! - `objc2-video-toolbox-0.3.2/.../VTSession.rs:14` `type VTSession = CFType`
//! - `objc2-core-media-0.3.2/.../CMFormatDescription.rs:664` kCMVideoCodecType_H264
//! - `objc2-core-media-0.3.2/.../CMFormatDescription.rs:1672` Get H.264 parameter set
//! - `objc2-core-media-0.3.2/.../CMBlockBuffer.rs:638` copy_data_bytes
//! - `objc2-core-video-0.3.2/.../CVPixelBuffer.rs:44` kCVPixelFormatType_32BGRA
//!
//! Output is **AVCC** (4-byte length-prefix NALUs) from VT. We convert to
//! Annex-B via `crate::h264_format::avcc_to_annexb` before returning — that's
//! what webrtc-rs `H264Payloader` expects. SPS/PPS live in the
//! `CMVideoFormatDescription` attached to the sample buffer, not embedded in
//! the AVCC payload; we extract them on the first keyframe.

#![cfg(all(feature = "h264", target_os = "macos"))]

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use bytes::Bytes;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_media::{
    kCMVideoCodecType_H264, CMSampleBuffer, CMTime, CMTimeFlags,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferCreateWithBytes};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_H264EntropyMode, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTH264EntropyMode_CABAC, kVTProfileLevel_H264_High_AutoLevel,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionSetProperty,
};
use tracing::{debug, info, warn};

use super::{BitrateBps, EncodedFrame, H264Encoder, ParameterSets};
use crate::h264_format::{avcc_to_annexb, build_avcc};

// ─── Shared state ───────────────────────────────────────────────────────────
//
// The VT compression callback fires on an internal VT thread. The encoder
// struct, owned by the capture loop, needs a safe way to receive completed
// frames. We keep an `Arc<Mutex<SharedState>>` — the callback pushes via a
// raw pointer cloned into the refcon; the encoder's `encode()` drains it
// synchronously after `encode_frame` returns.

#[derive(Default)]
struct SharedState {
    /// Encoded frames produced since the last `encode()` drain.
    /// VT may batch a small number of frames when it returns mid-GOP; we keep
    /// a VecDeque so we never lose one.
    completed: std::collections::VecDeque<EncodedFrame>,
    /// SPS + PPS extracted from the first IDR's format description. Cached
    /// so downstream layers can build the WebCodecs `avcC` description blob.
    params: Option<ParameterSets>,
}

// ─── Encoder ────────────────────────────────────────────────────────────────

pub struct VideoToolboxEncoder {
    session: CFRetained<VTCompressionSession>,
    bitrate: BitrateBps,
    state: Arc<Mutex<SharedState>>,
    /// Monotonic frame counter used for CMTime.value; the timescale is our
    /// constant `TIMESCALE_HZ` and gives value/timescale seconds since start.
    frame_counter: u64,
}

// VT compression sessions are documented as thread-safe by Apple; objc2's
// generated opaque types carry conservative `!Send + !Sync` because the CF
// runtime can't prove it for arbitrary types.
unsafe impl Send for VideoToolboxEncoder {}
unsafe impl Sync for VideoToolboxEncoder {}

/// Timescale for our PTS/duration CMTimes. 1_000_000 → µs granularity, which
/// fits comfortably in a CMTimeValue (i64). Apple's examples typically use
/// 600 or 90_000; µs works because VT only requires strictly-monotonic PTS.
const TIMESCALE_HZ: i32 = 1_000_000;
/// Microseconds per frame at 60 FPS. Real capture cadence may vary; VT's
/// rate control uses this only as a hint.
const DEFAULT_FRAME_DURATION_US: i64 = 16_666;

// ─── Session construction + properties ──────────────────────────────────────

fn build_encoder_spec() -> CFRetained<CFDictionary> {
    let key: &'static CFString =
        unsafe { kVTVideoEncoderSpecification_EnableLowLatencyRateControl };
    let value: &'static CFBoolean = CFBoolean::new(true);
    let typed: CFRetained<CFDictionary<CFString, CFBoolean>> =
        CFDictionary::from_slices(&[key], &[value]);
    unsafe { CFRetained::cast_unchecked::<CFDictionary>(typed) }
}

fn configure_properties(
    session: &VTCompressionSession,
    bitrate: BitrateBps,
) -> anyhow::Result<()> {
    unsafe {
        set_cf(session, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true))?;
        // High profile + AutoLevel: enables 8×8 adaptive transform + CABAC,
        // both critical for sharp text on screen content. AutoLevel keeps
        // the HiDPI safety from phase-01's `e7235a2` fix — Baseline 3.1
        // capped us at 1280×720 and HiDPI mode (2268×1473) tripped
        // kVTVideoEncoderMalfunctionErr (-12911) on every frame. AutoLevel
        // lets VT pick the right level (3.1 → 5.2) based on width/height.
        set_cf(
            session,
            kVTCompressionPropertyKey_ProfileLevel,
            kVTProfileLevel_H264_High_AutoLevel,
        )?;
        let bitrate_num = CFNumber::new_i32(bitrate.0 as i32);
        set_cf(session, kVTCompressionPropertyKey_AverageBitRate, &*bitrate_num)?;
        // 60 frames = 1 s @ 60 fps between forced IDRs. Shorter GOP than
        // phase-01's 120 frames halves the P-frame drift window — important
        // for crisp text on UI motion (scrolling, cursor). VT may emit
        // sooner on scene cuts or on client PLI. ~3% bitrate overhead trade.
        let kf_interval = CFNumber::new_i32(60);
        set_cf(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, &*kf_interval)?;
        // Belt-and-braces: also cap the time-based interval. Without this,
        // VT's RC may insert IDRs on detected scene changes — which fires
        // every frame in a same-machine recursive view, blowing the
        // bitrate budget on all-keyframe output. 2 s pairs cleanly with
        // the 60-frame cap above (whichever fires first wins).
        let kf_interval_s = CFNumber::new_f64(2.0);
        set_cf(
            session,
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
            &*kf_interval_s,
        )?;
        set_cf(
            session,
            kVTCompressionPropertyKey_AllowFrameReordering,
            CFBoolean::new(false),
        )?;
        // CABAC: ~10-15% bitrate efficiency win vs CAVLC at equal quality.
        // Requires Main+ profile; allowed here because we use High above.
        set_cf(
            session,
            kVTCompressionPropertyKey_H264EntropyMode,
            kVTH264EntropyMode_CABAC,
        )?;
    }
    Ok(())
}

unsafe fn set_cf<V>(
    session: &VTCompressionSession,
    key: &CFString,
    value: &V,
) -> anyhow::Result<()>
where
    V: AsRef<CFType> + ?Sized,
{
    let status = unsafe { VTSessionSetProperty(session, key, Some(value.as_ref())) };
    if status != 0 {
        anyhow::bail!("VTSessionSetProperty failed, OSStatus={}", status);
    }
    Ok(())
}

// ─── Compression output callback ────────────────────────────────────────────
//
// Called by VT (on its internal encode thread) once per successful compressed
// frame. We lock the shared state, extract the NAL bytes + keyframe flag +
// SPS/PPS (first IDR only), and push an `EncodedFrame`. Any failure is logged
// and the frame is silently dropped — misbehaving in the callback would at
// best leak, at worst crash.

unsafe extern "C-unwind" fn compression_output_callback(
    refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: refcon is an `Arc<Mutex<SharedState>>` that was `into_raw`'d at
    // session creation. It lives until the encoder's Drop runs the matching
    // `from_raw`. Borrowing as `&Mutex<...>` here is sound.
    let state: &Mutex<SharedState> = unsafe { &*(refcon as *const Mutex<SharedState>) };

    if status != 0 {
        warn!(status, "VT callback: frame encode failed");
        return;
    }
    let Some(sample) = (unsafe { sample_buffer.as_ref() }) else {
        return;
    };

    let pts_us = source_frame_refcon as u64;
    match extract_frame(sample, pts_us) {
        Ok((frame, maybe_params)) => {
            let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(params) = maybe_params {
                // First-IDR-only log: gated on `guard.params.is_none()` so a
                // 1 s GOP doesn't spam one info! per second. Verifies the
                // bitstream matches the SDP `profile-level-id` negotiation —
                // expected after the 2026-05-13 High profile bump:
                // profile_idc=100, level_idc per AutoLevel. SPS byte layout
                // (ITU-T H.264 §7.3.2.1): byte 0 = NAL header (0x67),
                // byte 1 = profile_idc, byte 2 = constraint_set flags,
                // byte 3 = level_idc.
                if guard.params.is_none() && params.sps.len() >= 4 {
                    info!(
                        profile_idc = params.sps[1],
                        level_idc = params.sps[3],
                        sps_len = params.sps.len(),
                        pps_len = params.pps.len(),
                        "VT first IDR: SPS extracted",
                    );
                }
                guard.params.get_or_insert(params);
            }
            guard.completed.push_back(frame);
        }
        Err(e) => warn!(error = %e, "VT callback: frame extraction failed"),
    }
}

/// Pull the AVCC NAL bytes out of a CMSampleBuffer, detect keyframe, and
/// (on keyframe) extract SPS+PPS from the attached format description.
/// Returns the fully-formed Annex-B `EncodedFrame`.
fn extract_frame(
    sample: &CMSampleBuffer,
    pts_us: u64,
) -> anyhow::Result<(EncodedFrame, Option<ParameterSets>)> {
    // Keyframe detection: attach-array is Option<CFRetained<CFArray>>; for a
    // keyframe, either the array is absent or every attachment lacks
    // NotSync=true. For Day-2 we rely on `is_keyframe_from_format_desc` which
    // is cheaper and equivalent for VT H.264.
    let fmt_desc = unsafe { sample.format_description() }
        .context("CMSampleBuffer missing format description")?;

    // If SPS/PPS count == 2 this is a keyframe; VT only attaches parameter
    // sets to IDRs. Non-IDR delta frames share the same fmt_desc across the
    // GOP but on first access (which coincides with first IDR) we extract.
    let mut ps_count: usize = 0;
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            &fmt_desc,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut ps_count,
            ptr::null_mut(),
        )
    };
    let maybe_params = if status == 0 && ps_count >= 2 {
        match (read_param_set(&fmt_desc, 0), read_param_set(&fmt_desc, 1)) {
            (Ok(sps), Ok(pps)) => Some(ParameterSets {
                sps: Bytes::copy_from_slice(&sps),
                pps: Bytes::copy_from_slice(&pps),
                // VT runs on the media engine; H264Encoder::is_hardware
                // returns true for this backend. Mirror that here so the
                // callback path doesn't have to re-probe.
                is_hardware: true,
            }),
            _ => None,
        }
    } else {
        None
    };


    let block_buf = unsafe { sample.data_buffer() }
        .context("CMSampleBuffer missing data buffer (H.264 sample expected)")?;
    let total_len = unsafe { block_buf.data_length() };
    if total_len == 0 {
        anyhow::bail!("CMBlockBuffer empty");
    }
    let mut avcc = vec![0u8; total_len];
    let copy_status = unsafe {
        block_buf.copy_data_bytes(
            0,
            total_len,
            NonNull::new_unchecked(avcc.as_mut_ptr() as *mut c_void),
        )
    };
    if copy_status != 0 {
        anyhow::bail!("CMBlockBufferCopyDataBytes failed, OSStatus={}", copy_status);
    }

    // Assemble Annex-B: for keyframes, prepend SPS+PPS as Annex-B so decoders
    // that drop the avcC description still sync on the bitstream alone. For
    // delta frames, just convert AVCC→Annex-B directly.
    let is_keyframe = maybe_params.is_some();
    let annexb = if let Some(p) = maybe_params.as_ref() {
        let body = avcc_to_annexb(&avcc);
        let mut out = Vec::with_capacity(8 + p.sps.len() + p.pps.len() + body.len());
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&p.sps);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&p.pps);
        out.extend_from_slice(&body);
        out
    } else {
        avcc_to_annexb(&avcc)
    };

    // `build_avcc` is re-exported so downstream can compute the WebCodecs
    // description blob from `ParameterSets` without re-parsing.
    let _ = &build_avcc;

    Ok((
        EncodedFrame {
            annexb: Bytes::from(annexb),
            is_keyframe,
            pts_us,
        },
        maybe_params,
    ))
}

fn read_param_set(
    fmt_desc: &objc2_core_media::CMFormatDescription,
    idx: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut ptr: *const u8 = ptr::null();
    let mut len: usize = 0;
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            fmt_desc,
            idx,
            &mut ptr,
            &mut len,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != 0 || ptr.is_null() || len == 0 {
        anyhow::bail!("param set {idx} extract failed (status={status}, len={len})");
    }
    // SAFETY: VT guarantees the pointer is valid while fmt_desc is retained;
    // we copy immediately so the Vec outlives the borrow.
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    Ok(slice.to_vec())
}

// ─── CVPixelBuffer construction for BGRA input ──────────────────────────────

/// VT's release callback signature: `(refcon: *mut c_void, base: *const c_void)`.
/// We pass a `Box<Vec<u8>>` as refcon so the BGRA copy is freed when VT is
/// done with the pixel buffer.
unsafe extern "C-unwind" fn bgra_release_cb(refcon: *mut c_void, _base: *const c_void) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: refcon was Box<Vec<u8>>::into_raw at create time.
    drop(unsafe { Box::from_raw(refcon as *mut Vec<u8>) });
}

fn wrap_bgra_as_pixel_buffer(
    bgra: &[u8],
    width: u32,
    height: u32,
) -> anyhow::Result<CFRetained<CVPixelBuffer>> {
    let bytes_per_row = (width as usize) * 4;
    anyhow::ensure!(
        bgra.len() >= bytes_per_row * (height as usize),
        "BGRA buffer too short: got {} bytes, need {}",
        bgra.len(),
        bytes_per_row * (height as usize)
    );

    // Copy — VT may hold the pixel buffer past encode_frame's return. The
    // copy is freed via `bgra_release_cb` when CV releases the wrapping
    // pixel buffer.
    let owned: Box<Vec<u8>> = Box::new(bgra[..bytes_per_row * height as usize].to_vec());
    let base_addr = owned.as_ptr() as *mut c_void;
    let refcon = Box::into_raw(owned) as *mut c_void;

    let mut pb_ptr: *mut CVPixelBuffer = ptr::null_mut();
    let ret = unsafe {
        CVPixelBufferCreateWithBytes(
            None,
            width as usize,
            height as usize,
            kCVPixelFormatType_32BGRA,
            NonNull::new_unchecked(base_addr),
            bytes_per_row,
            Some(bgra_release_cb),
            refcon,
            None,
            NonNull::new_unchecked(&mut pb_ptr),
        )
    };
    if ret != 0 {
        // Release our Box since CV won't call the release_cb on failure.
        unsafe { drop(Box::from_raw(refcon as *mut Vec<u8>)) };
        anyhow::bail!("CVPixelBufferCreateWithBytes failed, CVReturn={}", ret);
    }
    let nn = NonNull::new(pb_ptr).context("null CVPixelBuffer after successful create")?;
    Ok(unsafe { CFRetained::from_raw(nn) })
}

// ─── Public encoder API ─────────────────────────────────────────────────────

impl VideoToolboxEncoder {
    pub fn new(width: u32, height: u32, initial_bitrate: BitrateBps) -> anyhow::Result<Self> {
        anyhow::ensure!(
            width > 0 && height > 0 && width <= i32::MAX as u32 && height <= i32::MAX as u32,
            "invalid encoder dimensions {}x{}",
            width,
            height
        );
        let encoder_spec = build_encoder_spec();

        // Shared state — one Arc clone is leaked into VT as refcon, the other
        // lives in self. Drop rebalances by `Arc::from_raw`-ing the refcon.
        let state: Arc<Mutex<SharedState>> = Arc::new(Mutex::new(SharedState::default()));
        let refcon = Arc::into_raw(state.clone()) as *mut c_void;

        let mut session_ptr: *mut VTCompressionSession = ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                Some(&encoder_spec),
                None,
                None,
                Some(compression_output_callback),
                refcon,
                NonNull::new_unchecked(&mut session_ptr),
            )
        };
        if status != 0 {
            // Rebalance the leaked Arc before bailing.
            unsafe { drop(Arc::from_raw(refcon as *const Mutex<SharedState>)) };
            anyhow::bail!(
                "VTCompressionSessionCreate failed, OSStatus={} ({}x{})",
                status,
                width,
                height
            );
        }

        let session_nn = NonNull::new(session_ptr)
            .context("VTCompressionSessionCreate returned success but null pointer")?;
        let session = unsafe { CFRetained::from_raw(session_nn) };

        configure_properties(&session, initial_bitrate)?;

        Ok(Self {
            session,
            bitrate: initial_bitrate,
            state,
            frame_counter: 0,
        })
    }
}

impl Drop for VideoToolboxEncoder {
    fn drop(&mut self) {
        // Tear down the session first so no more callbacks will fire with our
        // refcon. Then drop the Arc clone that was leaked into VT as refcon.
        unsafe { self.session.invalidate() };
        let refcon = Arc::as_ptr(&self.state);
        // SAFETY: `refcon` is the same pointer Arc::into_raw produced in new().
        // We are the sole remaining clone route, and after invalidate() no VT
        // thread can still be inside the callback with this refcon.
        unsafe { drop(Arc::from_raw(refcon)) };
    }
}

impl H264Encoder for VideoToolboxEncoder {
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        let pixel_buf = wrap_bgra_as_pixel_buffer(bgra, width, height)?;

        // PTS and duration in our shared µs timescale. Value is frame_counter *
        // DEFAULT_FRAME_DURATION_US so PTS stays strictly monotonic across
        // capture jitter — VT only uses these for rate-control hints anyway.
        self.frame_counter = self.frame_counter.wrapping_add(1);
        let pts_us = self.frame_counter.saturating_mul(DEFAULT_FRAME_DURATION_US as u64);
        let pts = CMTime {
            value: pts_us as i64,
            timescale: TIMESCALE_HZ,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        let duration = CMTime {
            value: DEFAULT_FRAME_DURATION_US,
            timescale: TIMESCALE_HZ,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };

        // Build optional frame-property dict forcing an IDR when requested.
        let frame_props = force_idr.then(build_force_idr_dict);

        // The CV image buffer is &CVImageBuffer (= &CVBuffer); CVPixelBuffer
        // derefs to it via cf_type.
        let mut info_flags = VTEncodeInfoFlags::empty();
        let status = unsafe {
            self.session.encode_frame(
                &pixel_buf,
                pts,
                duration,
                frame_props.as_deref(),
                pts_us as *mut c_void,
                &mut info_flags,
            )
        };
        if status != 0 {
            anyhow::bail!("VTCompressionSessionEncodeFrame failed, OSStatus={}", status);
        }

        // Drain newest frame (if any). VT's callback fires synchronously for
        // sync-encoded frames and asynchronously for async; we don't wait —
        // returning Ok(None) just means no frame was ready yet, which the
        // trait contract permits.
        let frame = {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            guard.completed.pop_front()
        };
        if let Some(ref f) = frame {
            debug!(
                bytes = f.annexb.len(),
                is_keyframe = f.is_keyframe,
                "VT encoded frame"
            );
        }
        Ok(frame)
    }

    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()> {
        let num = CFNumber::new_i32(bitrate.0 as i32);
        unsafe {
            set_cf(&self.session, kVTCompressionPropertyKey_AverageBitRate, &*num)?;
        }
        self.bitrate = bitrate;
        Ok(())
    }

    fn parameter_sets(&self) -> Option<ParameterSets> {
        // SPS/PPS are populated on a VT callback thread into the shared
        // `Mutex<State>`; returning an owned clone sidesteps the MutexGuard
        // lifetime issue. Only the session layer calls this on first IDR.
        let guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        guard.params.clone()
    }

    fn is_hardware(&self) -> bool {
        // VideoToolbox runs on the Apple silicon / x86 Intel media engine.
        true
    }
}

impl VideoToolboxEncoder {
    #[allow(dead_code)]
    pub fn bitrate(&self) -> BitrateBps {
        self.bitrate
    }
}

/// Build the `frame_properties` dict that asks VT to emit the next frame as
/// an IDR. Keyed by `kVTEncodeFrameOptionKey_ForceKeyFrame = true`.
fn build_force_idr_dict() -> CFRetained<CFDictionary> {
    let key: &'static CFString =
        unsafe { objc2_video_toolbox::kVTEncodeFrameOptionKey_ForceKeyFrame };
    let val: &'static CFBoolean = CFBoolean::new(true);
    let typed: CFRetained<CFDictionary<CFString, CFBoolean>> =
        CFDictionary::from_slices(&[key], &[val]);
    unsafe { CFRetained::cast_unchecked::<CFDictionary>(typed) }
}
