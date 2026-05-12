//! VideoToolbox HEVC encoder — macOS hardware path.
//!
//! Mirrors `videotoolbox_encoder.rs` (H.264). Diverges: codec HEVC,
//! `kVTProfileLevel_HEVC_Main_AutoLevel`, VPS+SPS+PPS param sets,
//! no H264EntropyMode, extra MinAllowedFrameQP=1 + ExpectedFrameRate=60.

#![cfg(all(feature = "hevc", target_os = "macos"))]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use bytes::Bytes;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_media::{
    kCMVideoCodecType_HEVC, CMSampleBuffer, CMTime, CMTimeFlags,
    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxAllowedFrameQP,
    kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_MinAllowedFrameQP, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_Quality, kVTCompressionPropertyKey_RealTime,
    kVTProfileLevel_HEVC_Main_AutoLevel,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl, VTCompressionSession,
    VTEncodeInfoFlags,
};
use tracing::{debug, warn};

use super::vt_common::{
    avcc_to_annexb, build_force_idr_dict, set_cf, try_set_cf, wrap_bgra_as_pixel_buffer,
    DEFAULT_FRAME_DURATION_US, TIMESCALE_HZ,
};
use super::{BitrateBps, EncodedFrame, HevcEncoder, HevcParameterSets};

#[derive(Default)]
struct SharedStateHevc {
    completed: VecDeque<EncodedFrame>,
    params: Option<HevcParameterSets>,
}

pub struct VideoToolboxHevcEncoder {
    session: CFRetained<VTCompressionSession>,
    state: Arc<Mutex<SharedStateHevc>>,
    frame_counter: u64,
}

// VT sessions are thread-safe; conservative !Send on the opaque CF type is
// overridden here as for the H.264 encoder.
unsafe impl Send for VideoToolboxHevcEncoder {}
unsafe impl Sync for VideoToolboxHevcEncoder {}

fn build_encoder_spec() -> CFRetained<CFDictionary> {
    let key: &'static CFString = unsafe { kVTVideoEncoderSpecification_EnableLowLatencyRateControl };
    let val: &'static CFBoolean = CFBoolean::new(true);
    let typed: CFRetained<CFDictionary<CFString, CFBoolean>> = CFDictionary::from_slices(&[key], &[val]);
    unsafe { CFRetained::cast_unchecked::<CFDictionary>(typed) }
}

fn configure_properties_hevc(session: &VTCompressionSession, bitrate: BitrateBps, max_qp: Option<u32>) -> anyhow::Result<()> {
    unsafe {
        set_cf(session, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true))?;
        set_cf(session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main_AutoLevel)?;
        set_cf(session, kVTCompressionPropertyKey_AverageBitRate, &*CFNumber::new_i32(bitrate.0 as i32))?;
        set_cf(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, &*CFNumber::new_i32(600))?;
        set_cf(session, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, &*CFNumber::new_f64(30.0))?;
        set_cf(session, kVTCompressionPropertyKey_AllowFrameReordering, CFBoolean::new(false))?;
        // H264EntropyMode is H.264-only; HEVC is always CABAC internally.
        try_set_cf(session, kVTCompressionPropertyKey_Quality, &*CFNumber::new_f32(0.75), "Quality=0.75")?;
        if let Some(qp) = max_qp {
            try_set_cf(session, kVTCompressionPropertyKey_MaxAllowedFrameQP, &*CFNumber::new_i32(qp as i32), "MaxAllowedFrameQP")?;
        }
        try_set_cf(session, kVTCompressionPropertyKey_MinAllowedFrameQP, &*CFNumber::new_i32(1), "MinAllowedFrameQP=1")?;
        // Hardcoded 60 fps hint; parameterization from capture tier is a follow-up.
        try_set_cf(session, kVTCompressionPropertyKey_ExpectedFrameRate, &*CFNumber::new_i32(60), "ExpectedFrameRate=60")?;
    }
    Ok(())
}

unsafe extern "C-unwind" fn compression_output_callback_hevc(
    refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    if refcon.is_null() { return; }
    // SAFETY: refcon is Arc<Mutex<SharedStateHevc>> kept alive until Drop.
    let state: &Mutex<SharedStateHevc> = unsafe { &*(refcon as *const Mutex<SharedStateHevc>) };
    if status != 0 { warn!(status, "HEVC VT callback: encode failed"); return; }
    let Some(sample) = (unsafe { sample_buffer.as_ref() }) else { return };
    let pts_us = source_frame_refcon as u64;
    match extract_hevc_frame(sample, pts_us) {
        Ok((frame, params)) => {
            let mut g = state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(p) = params { g.params.get_or_insert(p); }
            g.completed.push_back(frame);
        }
        Err(e) => warn!(error = %e, "HEVC VT callback: extraction failed"),
    }
}

fn extract_hevc_frame(sample: &CMSampleBuffer, pts_us: u64) -> anyhow::Result<(EncodedFrame, Option<HevcParameterSets>)> {
    let fmt_desc = unsafe { sample.format_description() }.context("CMSampleBuffer missing format description")?;
    let mut ps_count: usize = 0;
    let probe_ok = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(&fmt_desc, 0, ptr::null_mut(), ptr::null_mut(), &mut ps_count, ptr::null_mut())
    } == 0;
    let maybe_params = if !probe_ok {
        None
    } else if ps_count < 3 {
        warn!(ps_count, "HEVC: ps_count < 3, treating as non-keyframe"); None
    } else {
        if ps_count > 3 { debug!(ps_count, "HEVC: extra param sets, using VPS/SPS/PPS (0-2)"); }
        match (read_hevc_ps(&fmt_desc, 0), read_hevc_ps(&fmt_desc, 1), read_hevc_ps(&fmt_desc, 2)) {
            (Ok(vps), Ok(sps), Ok(pps)) => Some(HevcParameterSets {
                vps: Bytes::copy_from_slice(&vps), sps: Bytes::copy_from_slice(&sps),
                pps: Bytes::copy_from_slice(&pps), is_hardware: true,
            }),
            _ => None,
        }
    };
    let block_buf = unsafe { sample.data_buffer() }.context("CMSampleBuffer missing data buffer")?;
    let total_len = unsafe { block_buf.data_length() };
    anyhow::ensure!(total_len > 0, "CMBlockBuffer empty");
    let mut avcc = vec![0u8; total_len];
    let st = unsafe { block_buf.copy_data_bytes(0, total_len, NonNull::new_unchecked(avcc.as_mut_ptr() as *mut c_void)) };
    anyhow::ensure!(st == 0, "CMBlockBufferCopyDataBytes failed, OSStatus={st}");
    let is_keyframe = maybe_params.is_some();
    let annexb = if let Some(ref p) = maybe_params {
        let body = avcc_to_annexb(&avcc);
        let mut out = Vec::with_capacity(12 + p.vps.len() + p.sps.len() + p.pps.len() + body.len());
        out.extend_from_slice(&[0,0,0,1]); out.extend_from_slice(&p.vps);
        out.extend_from_slice(&[0,0,0,1]); out.extend_from_slice(&p.sps);
        out.extend_from_slice(&[0,0,0,1]); out.extend_from_slice(&p.pps);
        out.extend_from_slice(&body); out
    } else { avcc_to_annexb(&avcc) };
    Ok((EncodedFrame { annexb: Bytes::from(annexb), is_keyframe, pts_us }, maybe_params))
}

fn read_hevc_ps(fmt_desc: &objc2_core_media::CMFormatDescription, idx: usize) -> anyhow::Result<Vec<u8>> {
    let mut p: *const u8 = ptr::null();
    let mut len: usize = 0;
    let st = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt_desc, idx, &mut p, &mut len, ptr::null_mut(), ptr::null_mut())
    };
    anyhow::ensure!(st == 0 && !p.is_null() && len > 0, "HEVC param set {idx} failed (st={st}, len={len})");
    // SAFETY: pointer valid while fmt_desc retained; copied immediately.
    Ok(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
}

impl VideoToolboxHevcEncoder {
    /// Build a new VT HEVC compression session.
    /// `max_qp`: quality floor (HEVC QP 0–51). HIGH=23, MED=28, LOW=None.
    pub fn new(width: u32, height: u32, initial_bitrate: BitrateBps, max_qp: Option<u32>) -> anyhow::Result<Self> {
        anyhow::ensure!(width > 0 && height > 0 && width <= i32::MAX as u32 && height <= i32::MAX as u32,
            "invalid encoder dimensions {}x{}", width, height);
        let spec = build_encoder_spec();
        let state: Arc<Mutex<SharedStateHevc>> = Arc::new(Mutex::new(SharedStateHevc::default()));
        let refcon = Arc::into_raw(state.clone()) as *mut c_void;
        let mut session_ptr: *mut VTCompressionSession = ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(None, width as i32, height as i32, kCMVideoCodecType_HEVC,
                Some(&spec), None, None, Some(compression_output_callback_hevc), refcon,
                NonNull::new_unchecked(&mut session_ptr))
        };
        if status != 0 {
            unsafe { drop(Arc::from_raw(refcon as *const Mutex<SharedStateHevc>)) };
            anyhow::bail!("VTCompressionSessionCreate (HEVC) failed, OSStatus={status} ({width}x{height})");
        }
        let nn = NonNull::new(session_ptr).context("VTCompressionSessionCreate (HEVC) returned null")?;
        let session = unsafe { CFRetained::from_raw(nn) };
        configure_properties_hevc(&session, initial_bitrate, max_qp)?;
        Ok(Self { session, state, frame_counter: 0 })
    }
}

impl Drop for VideoToolboxHevcEncoder {
    fn drop(&mut self) {
        // Invalidate before dropping refcon so no callback fires with a dangling pointer.
        unsafe { self.session.invalidate() };
        unsafe { drop(Arc::from_raw(Arc::as_ptr(&self.state))) };
    }
}

impl HevcEncoder for VideoToolboxHevcEncoder {
    fn encode(&mut self, bgra: &[u8], width: u32, height: u32, force_idr: bool) -> anyhow::Result<Option<EncodedFrame>> {
        let pixel_buf = wrap_bgra_as_pixel_buffer(bgra, width, height)?;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        let pts_us = self.frame_counter.saturating_mul(DEFAULT_FRAME_DURATION_US as u64);
        let pts = CMTime { value: pts_us as i64, timescale: TIMESCALE_HZ, flags: CMTimeFlags::Valid, epoch: 0 };
        let dur = CMTime { value: DEFAULT_FRAME_DURATION_US, timescale: TIMESCALE_HZ, flags: CMTimeFlags::Valid, epoch: 0 };
        let frame_props = force_idr.then(build_force_idr_dict);
        let mut info_flags = VTEncodeInfoFlags::empty();
        let status = unsafe {
            self.session.encode_frame(&pixel_buf, pts, dur, frame_props.as_deref(), pts_us as *mut c_void, &mut info_flags)
        };
        anyhow::ensure!(status == 0, "VTCompressionSessionEncodeFrame (HEVC) failed, OSStatus={status}");
        let frame = self.state.lock().unwrap_or_else(|p| p.into_inner()).completed.pop_front();
        if let Some(ref f) = frame { debug!(bytes = f.annexb.len(), is_keyframe = f.is_keyframe, "HEVC VT encoded frame"); }
        Ok(frame)
    }

    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()> {
        let num = CFNumber::new_i32(bitrate.0 as i32);
        unsafe { set_cf(&self.session, kVTCompressionPropertyKey_AverageBitRate, &*num)?; }
        Ok(())
    }

    fn parameter_sets(&self) -> Option<HevcParameterSets> {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).params.clone()
    }

    fn is_hardware(&self) -> bool { true }
}
