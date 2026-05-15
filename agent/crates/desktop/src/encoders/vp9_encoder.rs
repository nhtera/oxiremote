//! VP9 software encoder backend (libvpx, screen-content tuned).
//!
//! Configuration mirrors Chrome Remote Desktop's
//! `remoting/codec/webrtc_video_encoder_vpx.cc` so OxiRemote matches CRD
//! quality + speed on screen content. Key knobs:
//!   - `cpu-used = 6` (CRD default; range 5-9 trades CPU for quality)
//!   - `VP9E_SET_TUNE_CONTENT = VP9E_CONTENT_SCREEN`
//!   - `VP9E_SET_AQ_MODE = 3` (cyclic refresh — screen-content quantizer mode)
//!   - `VP9E_SET_ROW_MT = 1`, `VP9E_SET_TILE_COLUMNS = log2(threads)`
//!   - `rc_end_usage = VPX_CBR`
//!   - `kf_max_dist = 10000` (externally triggered keyframes only — pipeline
//!     runner force-IDRs via `force_idr=true` on PLI / 3s watchdog)
//!   - `g_lag_in_frames = 0` (no lookahead — real-time only)
//!
//! Active-map (16×16 dirty-rect skip) lands in phase-03; this phase ships
//! a full-frame encoder. The bitrate setter calls `vpx_codec_enc_config_set`
//! mid-stream; no encoder teardown.

use std::os::raw::{c_int, c_uint, c_ulong};
use std::ptr;
use std::time::SystemTime;

use anyhow::{anyhow, bail};
use bytes::Bytes;

use super::vpx_sys as vpx;
use super::{BitrateBps, EncodedFrame, Vp9Encoder, Vp9SequenceInfo};
use crate::capture::DirtyRect;

/// Phase-03b: 16×16 macroblock active-map buffer. Reused across frames
/// to avoid per-frame allocation. Stored as a flat row-major byte grid
/// (one byte per macroblock; 1 = encode, 0 = skip).
struct ActiveMap {
    buf: Vec<u8>,
    rows: u32,
    cols: u32,
}

impl ActiveMap {
    fn new(frame_width: u32, frame_height: u32) -> Self {
        // libvpx uses 16×16 macroblocks; ceil-divide so partial right/bottom
        // edges still get a block.
        let cols = frame_width.div_ceil(16);
        let rows = frame_height.div_ceil(16);
        Self {
            buf: vec![1u8; (rows * cols) as usize],
            rows,
            cols,
        }
    }

    /// Mark all blocks active (used on keyframes / cold start).
    fn mark_all(&mut self) {
        self.buf.fill(1);
    }

    /// Mark only the macroblocks intersecting `rects` as active.
    fn set_dirty(&mut self, rects: &[DirtyRect]) {
        self.buf.fill(0);
        for r in rects {
            let x0 = r.x / 16;
            let y0 = r.y / 16;
            // Right / bottom edges: ceil-divide so a single-pixel change at
            // the boundary still selects the touched block.
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

/// VP9 backend wrapping `vpx_codec_ctx_t`. Holds the I420 scratch buffer so
/// each encode call reuses the same allocation.
pub struct VpxVp9Encoder {
    ctx: vpx::vpx_codec_ctx_t,
    cfg: vpx::vpx_codec_enc_cfg_t,
    width: u32,
    height: u32,
    // BGRA buffer is converted into this scratch I420 buffer per frame.
    // Sized once at init (3/2 × width × height bytes).
    i420_buf: Vec<u8>,
    // PTS counter (ms-based timebase per CRD: g_timebase.num/.den = 1/1000).
    frame_index: u64,
    started_at: SystemTime,
    initialised: bool,
    // Phase-03b: per-frame 16×16 macroblock active-map. Lazy-built on the
    // first `apply_dirty_rects()` call so the encoder works correctly when
    // the pipeline doesn't drive active-map at all (old behaviour).
    active_map: Option<ActiveMap>,
}

// `vpx_codec_ctx_t` contains raw pointers; libvpx is thread-safe per-context
// but not Sync. We only ever own one instance per encoder thread.
unsafe impl Send for VpxVp9Encoder {}

impl VpxVp9Encoder {
    /// Initialise a new VP9 encoder for `width × height` BGRA input at the
    /// given starting bitrate.
    ///
    /// Returns an error if the libvpx symbols fail (codec init / config /
    /// control). Caller is expected to live on a dedicated blocking thread —
    /// libvpx blocks for the duration of `encode()`.
    pub fn new(width: u32, height: u32, initial_bitrate: BitrateBps) -> anyhow::Result<Self> {
        if width == 0 || height == 0 {
            bail!("vp9 encoder: zero-dim input ({width}x{height})");
        }
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            // libvpx requires even dims for I420 chroma sub-sampling.
            bail!("vp9 encoder: dims must be even (got {width}x{height})");
        }

        // SAFETY: libvpx initialises every field; zeroed struct is the
        // canonical pre-init state per `vpx_codec_enc_config_default`.
        let mut cfg: vpx::vpx_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        let iface = unsafe { vpx::vpx_codec_vp9_cx() };
        let rv = unsafe { vpx::vpx_codec_enc_config_default(iface, &mut cfg, 0) };
        if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
            bail!("vp9 encoder: vpx_codec_enc_config_default failed: {rv}");
        }

        // Override with CRD's screen-content config.
        cfg.g_w = width;
        cfg.g_h = height;
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 1000; // ms
        cfg.g_pass = vpx::vpx_enc_pass::VPX_RC_ONE_PASS;
        cfg.g_lag_in_frames = 0; // no lookahead
        cfg.g_threads = thread_count_for_width(width);
        cfg.kf_min_dist = 10_000;
        cfg.kf_max_dist = 10_000; // externally triggered keyframes only
        cfg.rc_dropframe_thresh = 0;
        cfg.rc_end_usage = vpx::vpx_rc_mode::VPX_CBR;
        cfg.rc_undershoot_pct = 100;
        cfg.rc_overshoot_pct = 15;
        cfg.rc_target_bitrate = (initial_bitrate.0 / 1000).max(50); // kbps, min 50
        cfg.g_profile = 0; // I420 — profile 1 (I444) deferred to a later phase
        cfg.g_error_resilient = 0;

        let mut ctx: vpx::vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let rv = unsafe {
            vpx::vpx_codec_enc_init_ver(
                &mut ctx,
                iface,
                &cfg,
                0,
                vpx::VPX_ENCODER_ABI_VERSION as c_int,
            )
        };
        if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
            bail!("vp9 encoder: vpx_codec_enc_init failed: {rv} — {}", codec_err_msg(&ctx));
        }

        // Post-init control calls — CRD's `SetVp9CodecOptions`.
        let threads = cfg.g_threads;
        let tile_columns = (threads as f32).log2().floor() as c_int;
        // Control IDs are values inside the `vp8e_enc_control_id` anonymous
        // enum module — bindgen places them there per the `ModuleConsts`
        // enum-style setting. Each control takes a c_int argument per
        // `VPX_CTRL_USE_TYPE(..., int)` in `vp8cx.h`.
        let controls: &[(c_int, c_int)] = &[
            (vpx::vp8e_enc_control_id::VP8E_SET_CPUUSED as c_int, 6),
            (vpx::vp8e_enc_control_id::VP9E_SET_TUNE_CONTENT as c_int, 1 /* VP9E_CONTENT_SCREEN */),
            (vpx::vp8e_enc_control_id::VP9E_SET_AQ_MODE as c_int, 3 /* cyclic refresh */),
            (vpx::vp8e_enc_control_id::VP9E_SET_ROW_MT as c_int, if threads > 1 { 1 } else { 0 }),
            (vpx::vp8e_enc_control_id::VP9E_SET_TILE_COLUMNS as c_int, tile_columns),
            (vpx::vp8e_enc_control_id::VP9E_SET_NOISE_SENSITIVITY as c_int, 0),
        ];
        for (ctrl_id, val) in controls {
            // SAFETY: control IDs map to known int-arg controls per
            // `vp8cx.h`; we feed `c_int` directly. The variadic
            // `vpx_codec_control_()` accepts an `int` for these IDs.
            let rv = unsafe { vpx::vpx_codec_control_(&mut ctx, *ctrl_id, *val) };
            if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
                let _ = unsafe { vpx::vpx_codec_destroy(&mut ctx) };
                bail!("vp9 encoder: control {ctrl_id} = {val} failed: {rv}");
            }
        }
        let _ = c_uint::default(); // silence unused-import if optimised away

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
                "vp9 encoder: BGRA buffer is {} bytes, expected {} ({}x{} × 4)",
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
        .map_err(|e| anyhow!("vp9 encoder: yuvutils-rs BGRA→I420 failed: {e:?}"))?;
        Ok(())
    }

    fn drain_packets(&mut self, force_idr: bool) -> anyhow::Result<Option<EncodedFrame>> {
        let mut iter: vpx::vpx_codec_iter_t = ptr::null();
        let mut payload = Vec::<u8>::new();
        let mut is_keyframe = false;

        loop {
            // SAFETY: vpx_codec_get_cx_data drives an internal iterator;
            // null `iter` on first call. Returned pointer lifetime is the
            // encoder context's — we copy the bytes immediately.
            let pkt = unsafe { vpx::vpx_codec_get_cx_data(&mut self.ctx, &mut iter) };
            if pkt.is_null() { break; }
            // SAFETY: pkt is non-null and points into the encoder context.
            // The union access reads `data.frame` only when `kind` says
            // it's a frame packet — that's the libvpx tagged-union
            // contract.
            unsafe {
                if (*pkt).kind == vpx::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    let frame = (*pkt).data.frame;
                    let bytes = std::slice::from_raw_parts(frame.buf as *const u8, frame.sz);
                    payload.extend_from_slice(bytes);
                    if frame.flags & vpx::VPX_FRAME_IS_KEY != 0 {
                        is_keyframe = true;
                    }
                }
            }
        }

        let _ = force_idr; // force_idr is consumed by encode flags, not by drain
        if payload.is_empty() {
            return Ok(None);
        }
        let pts_us = match self.started_at.elapsed() {
            Ok(d) => d.as_micros() as u64,
            Err(_) => {
                // System clock went backwards; fall back to a monotonic
                // approximation via the frame index at 30 fps.
                self.frame_index.saturating_mul(33_333)
            }
        };
        Ok(Some(EncodedFrame {
            annexb: Bytes::from(payload),
            is_keyframe,
            pts_us,
        }))
    }
}

impl Vp9Encoder for VpxVp9Encoder {
    fn encode(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force_idr: bool,
    ) -> anyhow::Result<Option<EncodedFrame>> {
        if width != self.width || height != self.height {
            bail!(
                "vp9 encoder: dim mismatch — encoder is {}x{}, got {}x{} \
                 (recreate encoder on capture size change)",
                self.width,
                self.height,
                width,
                height
            );
        }
        self.bgra_to_i420(bgra)?;

        // Wrap our owned I420 buffer as an `vpx_image_t`. `vpx_img_wrap`
        // does not take ownership — the buffer stays valid for the encode
        // call only, and we reuse it next frame.
        // SAFETY: img is fully populated by vpx_img_wrap; libvpx reads from
        // the wrapped buffer during vpx_codec_encode.
        let mut img: vpx::vpx_image_t = unsafe { std::mem::zeroed() };
        let wrapped = unsafe {
            vpx::vpx_img_wrap(
                &mut img,
                vpx::vpx_img_fmt::VPX_IMG_FMT_I420,
                self.width,
                self.height,
                1, // align
                self.i420_buf.as_mut_ptr(),
            )
        };
        if wrapped.is_null() {
            bail!("vp9 encoder: vpx_img_wrap returned null");
        }

        // Per CRD: ms-based PTS, 1ms duration per frame is fine — libvpx's
        // rate control only cares about the ratio of PTS deltas to bitrate.
        let pts_ms: vpx::vpx_codec_pts_t = self.frame_index.saturating_mul(33) as i64; // ~30 fps
        let flags: vpx::vpx_enc_frame_flags_t = if force_idr {
            vpx::VPX_EFLAG_FORCE_KF as vpx::vpx_enc_frame_flags_t
        } else {
            0
        };
        let rv = unsafe {
            vpx::vpx_codec_encode(
                &mut self.ctx,
                &img,
                pts_ms,
                33 as c_ulong, // ~30 fps duration in ms
                flags,
                vpx::VPX_DL_REALTIME as vpx::vpx_enc_deadline_t,
            )
        };
        if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
            bail!("vp9 encoder: vpx_codec_encode failed: {rv} — {}", codec_err_msg(&self.ctx));
        }
        self.frame_index = self.frame_index.saturating_add(1);
        self.drain_packets(force_idr)
    }

    fn set_bitrate(&mut self, bitrate: BitrateBps) -> anyhow::Result<()> {
        let new_kbps = (bitrate.0 / 1000).max(50);
        if new_kbps == self.cfg.rc_target_bitrate {
            return Ok(());
        }
        self.cfg.rc_target_bitrate = new_kbps;
        // SAFETY: cfg is the encoder's own config struct; vpx_codec_enc_config_set
        // copies the values it needs.
        let rv = unsafe { vpx::vpx_codec_enc_config_set(&mut self.ctx, &self.cfg) };
        if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
            bail!("vp9 encoder: vpx_codec_enc_config_set failed: {rv}");
        }
        Ok(())
    }

    fn sequence_info(&self) -> Option<Vp9SequenceInfo> {
        if self.initialised { Some(Vp9SequenceInfo { is_hardware: false }) } else { None }
    }

    fn is_hardware(&self) -> bool { false }

    /// Phase-03b: push dirty-rect-derived active map through
    /// `VP8E_SET_ACTIVEMAP`. CRD ground-truth: the same control ID (9)
    /// applies to both VP8 and VP9 in libvpx. Block size is fixed at
    /// 16×16. On keyframes (`force_full=true`) we mark every block active
    /// so the keyframe carries the full picture.
    fn apply_dirty_rects(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        rects: &[DirtyRect],
        force_full: bool,
    ) -> anyhow::Result<()> {
        if frame_width != self.width || frame_height != self.height {
            // Dim change in flight — the pipeline rebuilds the encoder on
            // the next frame; skip this call to avoid bogus dims.
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
        let mut am = vpx::vpx_active_map_t {
            active_map: map.buf.as_mut_ptr(),
            rows: map.rows,
            cols: map.cols,
        };
        let ctrl_id = vpx::vp8e_enc_control_id::VP8E_SET_ACTIVEMAP as c_int;
        // SAFETY: VP8E_SET_ACTIVEMAP requires `vpx_active_map_t*`. Pointer
        // lives for the duration of this control call; libvpx copies the
        // map internally before the next encode.
        let rv = unsafe {
            vpx::vpx_codec_control_(
                &mut self.ctx,
                ctrl_id,
                &mut am as *mut vpx::vpx_active_map_t,
            )
        };
        if rv != vpx::vpx_codec_err_t::VPX_CODEC_OK {
            bail!("vp9 encoder: VP8E_SET_ACTIVEMAP failed: {rv}");
        }
        Ok(())
    }
}

impl Drop for VpxVp9Encoder {
    fn drop(&mut self) {
        if self.initialised {
            // SAFETY: vpx_codec_destroy is idempotent post-init; we only
            // run it once via the `initialised` guard.
            unsafe { vpx::vpx_codec_destroy(&mut self.ctx) };
            self.initialised = false;
        }
    }
}

/// CRD's `GetEncoderThreadCount` formula, capped at `(num_cpus + 1) / 2`.
/// 1080p → 4 (cores allowing); 1440p → 8; 4K → 16.
fn thread_count_for_width(width: u32) -> c_uint {
    let bucket: u32 = if width >= 5120 {
        32
    } else if width >= 3840 {
        16
    } else if width >= 2560 {
        8
    } else if width >= 1280 {
        4
    } else if width >= 720 {
        2
    } else {
        1
    };
    let cap = num_cpus_or(4).div_ceil(2) as u32;
    bucket.min(cap).max(1)
}

/// Lightweight CPU-count probe — falls back to `default_cpus` on platforms
/// where `std::thread::available_parallelism` returns an error (very rare).
fn num_cpus_or(default_cpus: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(default_cpus)
}

/// Decode a libvpx error message from the context.
fn codec_err_msg(ctx: &vpx::vpx_codec_ctx_t) -> String {
    // SAFETY: vpx_codec_error returns a static / context-owned C string
    // pointer; CStr borrows it safely.
    unsafe {
        let ptr = vpx::vpx_codec_error(ctx as *const _ as *mut _);
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

    /// Synthesise a BGRA gradient buffer for `w × h`.
    fn make_bgra(w: u32, h: u32, frame_index: u8) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                buf[idx] = ((x + frame_index as u32) & 0xFF) as u8; // B
                buf[idx + 1] = ((y + frame_index as u32) & 0xFF) as u8; // G
                buf[idx + 2] = (frame_index as u32 & 0xFF) as u8; // R
                buf[idx + 3] = 0xFF; // A
            }
        }
        buf
    }

    #[test]
    fn encodes_keyframe_then_delta_frames() {
        let w = 320;
        let h = 240;
        let mut enc = VpxVp9Encoder::new(w, h, BitrateBps::LOW).expect("encoder init");

        // Frame 0: keyframe (libvpx always emits KF as first output).
        let f0 = enc
            .encode(&make_bgra(w, h, 0), w, h, true /* force_idr */)
            .expect("encode frame 0")
            .expect("frame 0 must produce output");
        assert!(f0.is_keyframe, "first encoded frame must be a keyframe");
        assert!(!f0.annexb.is_empty(), "keyframe payload must be non-empty");

        // Frames 1-3: P-frames (no forced KF).
        for i in 1..4u8 {
            let bgra = make_bgra(w, h, i);
            let f = enc
                .encode(&bgra, w, h, false)
                .expect(&format!("encode frame {i}"));
            if let Some(frame) = f {
                if i > 0 {
                    assert!(
                        !frame.is_keyframe,
                        "frame {i} should not be a keyframe (kf_max_dist=10000)"
                    );
                }
            }
            // libvpx may emit no output for some delta frames under CBR + screen-content
            // when the content is too similar; that's OK for this smoke test.
        }
    }

    #[test]
    fn rejects_odd_dimensions() {
        let r = VpxVp9Encoder::new(321, 240, BitrateBps::LOW);
        match r {
            Err(e) => assert!(e.to_string().contains("even"), "error must mention even dims: {e}"),
            Ok(_) => panic!("odd width must fail to init encoder"),
        }
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(VpxVp9Encoder::new(0, 240, BitrateBps::LOW).is_err());
        assert!(VpxVp9Encoder::new(320, 0, BitrateBps::LOW).is_err());
    }

    #[test]
    fn set_bitrate_updates_config_without_teardown() {
        let mut enc = VpxVp9Encoder::new(320, 240, BitrateBps::LOW).expect("encoder init");
        enc.set_bitrate(BitrateBps::HIGH).expect("set_bitrate must succeed");
        // Encode a frame after rate change — must still succeed.
        let f = enc
            .encode(&make_bgra(320, 240, 0), 320, 240, true)
            .expect("encode after rate change")
            .expect("frame produced after rate change");
        assert!(f.is_keyframe);
    }

    #[test]
    fn dim_mismatch_returns_error() {
        let mut enc = VpxVp9Encoder::new(320, 240, BitrateBps::LOW).expect("init");
        let bgra = make_bgra(640, 480, 0);
        let e = enc.encode(&bgra, 640, 480, true).expect_err("dim mismatch must error");
        assert!(e.to_string().contains("dim mismatch"));
    }

    #[test]
    fn active_map_marks_only_dirty_blocks() {
        // 64×64 frame → 4×4 macroblocks. One dirty rect covering the
        // top-left 16×16 must mark exactly (0,0) active.
        let mut m = ActiveMap::new(64, 64);
        assert_eq!(m.rows, 4);
        assert_eq!(m.cols, 4);
        m.set_dirty(&[DirtyRect { x: 0, y: 0, width: 16, height: 16 }]);
        assert_eq!(m.buf[0], 1, "top-left block must be active");
        assert_eq!(m.buf[1], 0, "block (0,1) must be skipped");
        assert_eq!(m.buf[4], 0, "block (1,0) must be skipped");
    }

    #[test]
    fn active_map_partial_block_overlap_marks_block() {
        // 1px change at boundary still selects the touched 16×16 block.
        let mut m = ActiveMap::new(32, 32); // 2×2 blocks
        m.set_dirty(&[DirtyRect { x: 15, y: 15, width: 2, height: 2 }]);
        // The rect spans (15..17, 15..17) — touches blocks (0,0) and (1,1).
        assert_eq!(m.buf[0], 1, "block (0,0) touched");
        assert_eq!(m.buf[3], 1, "block (1,1) touched (col=1, row=1)");
    }

    #[test]
    fn active_map_mark_all_resets_to_full() {
        let mut m = ActiveMap::new(32, 32);
        m.set_dirty(&[]); // clears
        assert!(m.buf.iter().all(|&b| b == 0), "set_dirty empty → all zero");
        m.mark_all();
        assert!(m.buf.iter().all(|&b| b == 1), "mark_all → all one");
    }

    #[test]
    fn thread_count_formula_matches_crd() {
        // CRD's formula, with our (cores+1)/2 cap. On a typical 8-core M-series,
        // cap is 4. Verify bucket assignments.
        let _ = thread_count_for_width(640);  // bucket = 1
        let _ = thread_count_for_width(1280); // bucket = 4
        let _ = thread_count_for_width(1920); // bucket = 4
        let _ = thread_count_for_width(3840); // bucket = 16 (capped)
        // Just smoke — we don't assert exact values because they depend on
        // local CPU core count.
        assert!(thread_count_for_width(640) >= 1);
        assert!(thread_count_for_width(1920) >= 2);
    }
}
