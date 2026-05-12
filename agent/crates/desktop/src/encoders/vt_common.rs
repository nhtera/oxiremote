//! Shared VideoToolbox helpers used by both H.264 and HEVC encoders.
//!
//! Extracted verbatim from `videotoolbox_encoder.rs` so the two encoder
//! backends can share pixel-buffer wrapping, force-IDR dict, and
//! property-set helpers without duplication.

#![cfg(all(any(feature = "h264", feature = "hevc"), target_os = "macos"))]

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use anyhow::Context;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_video::{kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferCreateWithBytes};
use objc2_video_toolbox::{kVTEncodeFrameOptionKey_ForceKeyFrame, VTCompressionSession, VTSessionSetProperty};
use tracing::debug;

/// Timescale for PTS/duration CMTimes. 1_000_000 → µs granularity, which
/// fits comfortably in a CMTimeValue (i64).
pub(crate) const TIMESCALE_HZ: i32 = 1_000_000;

/// Microseconds per frame at 60 FPS. VT rate control uses this only as a hint.
pub(crate) const DEFAULT_FRAME_DURATION_US: i64 = 16_666;

/// Set a required CF property on a VT session; bail on any non-zero OSStatus.
pub(crate) unsafe fn set_cf<V>(
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

/// Like [`set_cf`] but tolerates `kVTPropertyNotSupportedErr` (-12900).
///
/// Use for optional encoder properties that may not be implemented by every
/// VT encoder variant. `desc` is included in the log/error message.
pub(crate) unsafe fn try_set_cf<V>(
    session: &VTCompressionSession,
    key: &CFString,
    value: &V,
    desc: &'static str,
) -> anyhow::Result<()>
where
    V: AsRef<CFType> + ?Sized,
{
    let status = unsafe { VTSessionSetProperty(session, key, Some(value.as_ref())) };
    match status {
        0 => Ok(()),
        // kVTPropertyNotSupportedErr — encoder doesn't implement this knob;
        // not fatal, continue without it.
        -12900 => {
            debug!(property = desc, "VT property not supported by encoder, skipping");
            Ok(())
        }
        other => anyhow::bail!("VTSessionSetProperty({desc}) failed, OSStatus={other}"),
    }
}

/// VT release callback: frees the `Box<Vec<u8>>` BGRA copy when CV releases
/// the pixel buffer.
pub(crate) unsafe extern "C-unwind" fn bgra_release_cb(
    refcon: *mut c_void,
    _base: *const c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: refcon was Box<Vec<u8>>::into_raw at create time.
    drop(unsafe { Box::from_raw(refcon as *mut Vec<u8>) });
}

/// Wrap a BGRA slice as a `CVPixelBuffer`, copying the bytes so VT can hold
/// the buffer past `encode_frame`'s return.
pub(crate) fn wrap_bgra_as_pixel_buffer(
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

/// Build the frame-properties dict that requests VT to emit the next frame as
/// an IDR (keyed by `kVTEncodeFrameOptionKey_ForceKeyFrame = true`).
pub(crate) fn build_force_idr_dict() -> CFRetained<CFDictionary> {
    let key: &'static CFString = unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame };
    let val: &'static CFBoolean = CFBoolean::new(true);
    let typed: CFRetained<CFDictionary<CFString, CFBoolean>> =
        CFDictionary::from_slices(&[key], &[val]);
    unsafe { CFRetained::cast_unchecked::<CFDictionary>(typed) }
}

/// Convert AVCC (4-byte length-prefix) NALUs to Annex-B (start-code prefix).
///
/// The H.264 path uses `crate::h264_format::avcc_to_annexb` instead.
/// This copy is only needed by the HEVC path so it doesn't depend on `h264`.
#[cfg(feature = "hevc")]
pub(crate) fn avcc_to_annexb(avcc: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(avcc.len() + avcc.len() / 8);
    let mut pos = 0;
    while pos + 4 <= avcc.len() {
        let n = u32::from_be_bytes([avcc[pos], avcc[pos+1], avcc[pos+2], avcc[pos+3]]) as usize;
        pos += 4;
        if n == 0 || pos + n > avcc.len() { break; }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[pos..pos + n]);
        pos += n;
    }
    out
}
