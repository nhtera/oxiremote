//! AV1 software encoder backend (libaom, screen-content tuned).
//!
//! Mirrors `vp9_encoder.rs` but targets libaom's AV1 encoder. Configuration
//! matches Chrome Remote Desktop's `remoting/codec/webrtc_video_encoder_av1.cc`:
//!   - `g_usage = AOM_USAGE_REALTIME` (must be set BEFORE control calls)
//!   - `AOME_SET_CPUUSED = 11` (libaom's RT mode)
//!   - `AV1E_SET_TUNE_CONTENT = AOM_CONTENT_SCREEN` (palette + IBC — the
//!     critical screen-content quality lever; SVT-AV1 + rav1e both lack this)
//!   - `rc_min_quantizer = 10`, `rc_max_quantizer = 52`
//!   - `g_lag_in_frames = 0` (no lookahead)
//!   - `kf_mode = AOM_KF_DISABLED` (externally triggered keyframes only)
//!   - `rc_end_usage = AOM_CBR`
//!   - `AV1E_SET_ROW_MT = 1`, `AV1E_SET_TILE_COLUMNS = log2(threads)`,
//!     `AV1E_SET_TILE_ROWS = log2(threads)`
//!
//! Phase-05 ships the encoder. The RTP payloader (RFC 9606 OBU
//! fragmentation) lands in phase-06, and the desktop_ws.rs Pipeline::Av1
//! dispatch arm wires it together.

use std::os::raw::{c_int, c_ulong, c_uint};
use std::ptr;
use std::time::SystemTime;

use anyhow::{anyhow, bail};
use bytes::Bytes;

use super::aom_sys as aom;
use super::{Av1Encoder, Av1SequenceInfo, BitrateBps, EncodedFrame};
use crate::capture::DirtyRect;

/// Phase-03b: 16×16 macroblock active-map (mirrors `vp9_encoder::ActiveMap`).
struct ActiveMap {
    buf: Vec<u8>,
    rows: u32,
    cols: u32,
}

impl ActiveMap {
    fn new(frame_width: u32, frame_height: u32) -> Self {
        let cols = frame_width.div_ceil(16);
        let rows = frame_height.div_ceil(16);
        Self {
            buf: vec![1u8; (rows * cols) as usize],
            rows,
            cols,
        }
    }
    fn mark_all(&mut self) {
        self.buf.fill(1);
    }
    fn set_dirty(&mut self, rects: &[DirtyRect]) {
        self.buf.fill(0);
        for r in rects {
            let x0 = r.x / 16;
            let y0 = r.y / 16;
            let x1 = (r.x + r.width).div_ceil(16).min(self.cols);
            let y1 = (r.y + r.height).div_ceil(16).min(self.rows);
            for y in y0..y1 {
                let row = (y * self.cols) as usize;
                for x in x0..x1 {
                    if let Some(slot) = self.buf.get_mut(row + x as usize) {
                        *slot = 1;
                    }
                }
            }
        }
    }
}

/// AV1 backend wrapping `aom_codec_ctx_t`.
pub struct AomAv1Encoder {
    ctx: aom::aom_codec_ctx_t,
    cfg: aom::aom_codec_enc_cfg_t,
    width: u32,
    height: u32,
    i420_buf: Vec<u8>,
    frame_index: u64,
    started_at: SystemTime,
    initialised: bool,
    /// Phase-03b: lazy-built active-map reused across frames.
    active_map: Option<ActiveMap>,
}

unsafe impl Send for AomAv1Encoder {}

impl AomAv1Encoder {
    /// Initialise libaom AV1 for `width × height` BGRA input at the given
    /// starting bitrate.
    pub fn new(width: u32, height: u32, initial_bitrate: BitrateBps) -> anyhow::Result<Self> {
        if width == 0 || height == 0 {
            bail!("av1 encoder: zero-dim input ({width}x{height})");
        }
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("av1 encoder: dims must be even (got {width}x{height})");
        }

        // SAFETY: libaom initialises every field of `cfg`; zeroed pre-init is
        // canonical per `aom_codec_enc_config_default(..., usage=REALTIME)`.
        let mut cfg: aom::aom_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        let iface = unsafe { aom::aom_codec_av1_cx() };
        let rv = unsafe {
            aom::aom_codec_enc_config_default(iface, &mut cfg, aom::AOM_USAGE_REALTIME)
        };
        if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
            bail!("av1 encoder: aom_codec_enc_config_default failed: {rv}");
        }

        // Apply CRD's RT screen-content config on top of REALTIME defaults.
        cfg.g_w = width;
        cfg.g_h = height;
        cfg.g_usage = aom::AOM_USAGE_REALTIME;
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 1000; // ms
        cfg.g_pass = aom::aom_enc_pass::AOM_RC_ONE_PASS;
        cfg.g_lag_in_frames = 0;
        cfg.g_threads = thread_count_for_dim(width.max(height));
        cfg.g_error_resilient = 0;
        cfg.kf_mode = aom::aom_kf_mode::AOM_KF_DISABLED;
        cfg.rc_end_usage = aom::aom_rc_mode::AOM_CBR;
        cfg.rc_min_quantizer = 10;
        cfg.rc_max_quantizer = 52;
        cfg.rc_dropframe_thresh = 0;
        cfg.rc_undershoot_pct = 25;
        cfg.rc_overshoot_pct = 25;
        cfg.rc_target_bitrate = (initial_bitrate.0 / 1000).max(50);

        let mut ctx: aom::aom_codec_ctx_t = unsafe { std::mem::zeroed() };
        let rv = unsafe {
            aom::aom_codec_enc_init_ver(
                &mut ctx,
                iface,
                &cfg,
                0,
                aom::AOM_ENCODER_ABI_VERSION as c_int,
            )
        };
        if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
            bail!("av1 encoder: aom_codec_enc_init failed: {rv} — {}", codec_err_msg(&ctx));
        }

        // Post-init control calls: cpu_used=11 (RT), screen content tuning,
        // row-MT, tile cols/rows = log2(threads). Each control's required
        // arg type per `AOM_CTRL_USE_TYPE` in `aomcx.h` is encoded in the
        // value-as-c_int below; for u32 controls we transmute via `as c_int`.
        let threads = cfg.g_threads;
        let tile_log2 = (threads as f32).log2().floor().max(0.0) as c_int;
        // `AOM_CONTENT_SCREEN` is the second value in the `aom_tune_content`
        // enum (DEFAULT=0, SCREEN=1). The constant lives in the
        // `aom_tune_content` module per bindgen's enum-style.
        let av1e_set_tune_content =
            aom::aome_enc_control_id::AV1E_SET_TUNE_CONTENT as c_int;
        let av1e_set_row_mt = aom::aome_enc_control_id::AV1E_SET_ROW_MT as c_int;
        let av1e_set_tile_cols =
            aom::aome_enc_control_id::AV1E_SET_TILE_COLUMNS as c_int;
        let av1e_set_tile_rows =
            aom::aome_enc_control_id::AV1E_SET_TILE_ROWS as c_int;
        let aome_set_cpuused = aom::aome_enc_control_id::AOME_SET_CPUUSED as c_int;
        let aom_content_screen = aom::aom_tune_content::AOM_CONTENT_SCREEN as c_int;

        let controls: &[(c_int, c_int)] = &[
            (aome_set_cpuused, 11), // libaom RT mode
            (av1e_set_tune_content, aom_content_screen),
            (av1e_set_row_mt, if threads > 1 { 1 } else { 0 }),
            (av1e_set_tile_cols, tile_log2),
            (av1e_set_tile_rows, tile_log2),
        ];
        for (ctrl_id, val) in controls {
            // SAFETY: control IDs map to int-arg controls per `aomcx.h`
            // `AOM_CTRL_USE_TYPE(..., int|unsigned int)`. The variadic
            // `aom_codec_control` accepts an `int` and libaom widens
            // internally for u32 controls.
            let rv = unsafe { aom::aom_codec_control(&mut ctx, *ctrl_id, *val) };
            if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
                let _ = unsafe { aom::aom_codec_destroy(&mut ctx) };
                bail!("av1 encoder: control {ctrl_id} = {val} failed: {rv}");
            }
        }
        let _ = c_uint::default();

        let i420_size = (width as usize) * (height as usize) * 3 / 2;
        Ok(Self {
            ctx,
            cfg,
            width,
            height,
            i420_buf: vec![0u8; i420_size],
            frame_index: 0,
            started_at: SystemTime::now(),
            initialised: true,
            active_map: None,
        })
    }

    fn bgra_to_i420(&mut self, bgra: &[u8]) -> anyhow::Result<()> {
        let expected = (self.width as usize) * (self.height as usize) * 4;
        if bgra.len() != expected {
            bail!(
                "av1 encoder: BGRA buffer is {} bytes, expected {} ({}x{} × 4)",
                bgra.len(),
                expected,
                self.width,
                self.height
            );
        }
        let w = self.width as usize;
        let h = self.height as usize;
        let y_size = w * h;
        let uv_w = w / 2;
        let uv_h = h / 2;
        let (y, rest) = self.i420_buf.split_at_mut(y_size);
        let (u, v) = rest.split_at_mut(uv_w * uv_h);

        yuvutils_rs::bgra_to_yuv420(
            &mut yuvutils_rs::YuvPlanarImageMut {
                y_plane: yuvutils_rs::BufferStoreMut::Borrowed(y),
                u_plane: yuvutils_rs::BufferStoreMut::Borrowed(u),
                v_plane: yuvutils_rs::BufferStoreMut::Borrowed(v),
                y_stride: w as u32,
                u_stride: uv_w as u32,
                v_stride: uv_w as u32,
                width: self.width,
                height: self.height,
            },
            bgra,
            (w * 4) as u32,
            yuvutils_rs::YuvRange::Limited,
            yuvutils_rs::YuvStandardMatrix::Bt709,
            yuvutils_rs::YuvConversionMode::Balanced,
        )
        .map_err(|e| anyhow!("av1 encoder: yuvutils-rs BGRA→I420 failed: {e:?}"))?;
        Ok(())
    }

    fn drain_packets(&mut self) -> anyhow::Result<Option<EncodedFrame>> {
        let mut iter: aom::aom_codec_iter_t = ptr::null();
        let mut payload = Vec::<u8>::new();
        let mut is_keyframe = false;
        loop {
            let pkt = unsafe { aom::aom_codec_get_cx_data(&mut self.ctx, &mut iter) };
            if pkt.is_null() {
                break;
            }
            // SAFETY: pkt is non-null and points into the encoder context.
            unsafe {
                if (*pkt).kind == aom::aom_codec_cx_pkt_kind::AOM_CODEC_CX_FRAME_PKT {
                    let frame = (*pkt).data.frame;
                    let bytes = std::slice::from_raw_parts(frame.buf as *const u8, frame.sz);
                    payload.extend_from_slice(bytes);
                    if frame.flags & aom::AOM_FRAME_IS_KEY != 0 {
                        is_keyframe = true;
                    }
                }
            }
        }

        if payload.is_empty() {
            return Ok(None);
        }
        let pts_us = match self.started_at.elapsed() {
            Ok(d) => d.as_micros() as u64,
            Err(_) => self.frame_index.saturating_mul(33_333),
        };
        Ok(Some(EncodedFrame {
            annexb: Bytes::from(payload),
            is_keyframe,
            pts_us,
        }))
    }
}

impl Av1Encoder for AomAv1Encoder {
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        if width != self.width || height != self.height {
            bail!(
                "av1 encoder: dim mismatch — encoder is {}x{}, got {}x{}",
                self.width,
                self.height,
                width,
                height
            );
        }
        self.bgra_to_i420(bgra)?;

        // SAFETY: img is populated by aom_img_wrap; libaom reads from the
        // wrapped buffer for the duration of aom_codec_encode.
        let mut img: aom::aom_image_t = unsafe { std::mem::zeroed() };
        let wrapped = unsafe {
            aom::aom_img_wrap(
                &mut img,
                aom::aom_img_fmt::AOM_IMG_FMT_I420,
                self.width,
                self.height,
                1,
                self.i420_buf.as_mut_ptr(),
            )
        };
        if wrapped.is_null() {
            bail!("av1 encoder: aom_img_wrap returned null");
        }

        let pts_ms: aom::aom_codec_pts_t = self.frame_index.saturating_mul(33) as i64;
        let flags: aom::aom_enc_frame_flags_t = if force_idr {
            aom::AOM_EFLAG_FORCE_KF as aom::aom_enc_frame_flags_t
        } else {
            0
        };
        let rv = unsafe {
            aom::aom_codec_encode(
                &mut self.ctx,
                &img,
                pts_ms,
                33 as c_ulong,
                flags,
            )
        };
        if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
            bail!("av1 encoder: aom_codec_encode failed: {rv} — {}", codec_err_msg(&self.ctx));
        }
        self.frame_index = self.frame_index.saturating_add(1);
        self.drain_packets()
    }

    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()> {
        let new_kbps = (bitrate.0 / 1000).max(50);
        if new_kbps == self.cfg.rc_target_bitrate {
            return Ok(());
        }
        self.cfg.rc_target_bitrate = new_kbps;
        let rv = unsafe { aom::aom_codec_enc_config_set(&mut self.ctx, &self.cfg) };
        if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
            bail!("av1 encoder: aom_codec_enc_config_set failed: {rv}");
        }
        Ok(())
    }

    fn sequence_info(&self) -> Option<Av1SequenceInfo> {
        if self.initialised { Some(Av1SequenceInfo { is_hardware: false }) } else { None }
    }

    fn is_hardware(&self) -> bool { false }

    /// Phase-03b: push the dirty-rect-derived active map through
    /// `AOME_SET_ACTIVEMAP` (control ID 9 — shared with VP8/VP9). Block
    /// size is 16×16. `force_full=true` (keyframes / cold start) marks
    /// every block active so libaom encodes the full frame.
    fn apply_dirty_rects(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        rects: &[DirtyRect],
        force_full: bool,
    ) -> anyhow::Result<()> {
        if frame_width != self.width || frame_height != self.height {
            return Ok(());
        }
        let map = self
            .active_map
            .get_or_insert_with(|| ActiveMap::new(self.width, self.height));
        if force_full || rects.is_empty() {
            map.mark_all();
        } else {
            map.set_dirty(rects);
        }
        let mut am = aom::aom_active_map_t {
            active_map: map.buf.as_mut_ptr(),
            rows: map.rows,
            cols: map.cols,
        };
        let ctrl_id = aom::aome_enc_control_id::AOME_SET_ACTIVEMAP as c_int;
        // SAFETY: AOME_SET_ACTIVEMAP requires `aom_active_map_t*`. libaom
        // copies the map before the next encode.
        let rv = unsafe {
            aom::aom_codec_control(
                &mut self.ctx,
                ctrl_id,
                &mut am as *mut aom::aom_active_map_t,
            )
        };
        if rv != aom::aom_codec_err_t::AOM_CODEC_OK {
            bail!("av1 encoder: AOME_SET_ACTIVEMAP failed: {rv}");
        }
        Ok(())
    }
}

impl Drop for AomAv1Encoder {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { aom::aom_codec_destroy(&mut self.ctx) };
            self.initialised = false;
        }
    }
}

/// libaom thread count by max dimension. Same formula as libvpx (VP9) per
/// CRD's shared `GetEncoderThreadCount`. Capped at `(cpus+1)/2`.
fn thread_count_for_dim(dim: u32) -> c_uint {
    let bucket: u32 = if dim >= 5120 {
        32
    } else if dim >= 3840 {
        16
    } else if dim >= 2560 {
        8
    } else if dim >= 1280 {
        4
    } else if dim >= 720 {
        2
    } else {
        1
    };
    let cap = num_cpus_or(4).div_ceil(2) as u32;
    bucket.min(cap).max(1)
}

fn num_cpus_or(default_cpus: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(default_cpus)
}

fn codec_err_msg(ctx: &aom::aom_codec_ctx_t) -> String {
    unsafe {
        let ptr = aom::aom_codec_error(ctx as *const _ as *mut _);
        if ptr.is_null() {
            "<no error string>".to_string()
        } else {
            std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bgra(w: u32, h: u32, idx: u8) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                buf[i] = ((x + idx as u32) & 0xFF) as u8;
                buf[i + 1] = ((y + idx as u32) & 0xFF) as u8;
                buf[i + 2] = (idx as u32 & 0xFF) as u8;
                buf[i + 3] = 0xFF;
            }
        }
        buf
    }

    #[test]
    fn encodes_keyframe_on_force_idr() {
        let w = 320;
        let h = 240;
        let mut enc = AomAv1Encoder::new(w, h, BitrateBps::LOW).expect("init");
        let f = enc
            .encode(&make_bgra(w, h, 0), w, h, true)
            .expect("encode")
            .expect("frame 0 must produce output");
        assert!(f.is_keyframe, "first frame with force_idr=true must be KF");
        assert!(!f.annexb.is_empty(), "KF payload must be non-empty");
    }

    #[test]
    fn rejects_odd_dimensions() {
        let r = AomAv1Encoder::new(321, 240, BitrateBps::LOW);
        match r {
            Err(e) => assert!(e.to_string().contains("even"), "{e}"),
            Ok(_) => panic!("odd width must fail"),
        }
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(AomAv1Encoder::new(0, 240, BitrateBps::LOW).is_err());
        assert!(AomAv1Encoder::new(320, 0, BitrateBps::LOW).is_err());
    }

    #[test]
    fn set_bitrate_updates_config() {
        let mut enc = AomAv1Encoder::new(320, 240, BitrateBps::LOW).expect("init");
        enc.set_bitrate(BitrateBps::HIGH).expect("set_bitrate");
    }

    #[test]
    fn dim_mismatch_returns_error() {
        let mut enc = AomAv1Encoder::new(320, 240, BitrateBps::LOW).expect("init");
        let big = make_bgra(640, 480, 0);
        let e = enc.encode(&big, 640, 480, true).expect_err("dim mismatch must error");
        assert!(e.to_string().contains("dim mismatch"));
    }
}
