/// Tile-diff encoder: resize → xxhash tile comparison → libjpeg-turbo JPEG encode.
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{FilterType as FirFilter, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::RgbaImage;
use rayon::prelude::*;
use turbojpeg::{Compressor, Image as TjImage, PixelFormat as TjPixel, Subsamp};
use xxhash_rust::xxh3::xxh3_64;

/// JPEG quality per tier (0–100). High bumped to 90 for sharper text on
/// retina hosts at the same bandwidth tier (turbojpeg's 4:4:4 chroma at
/// q=90 is close to visually lossless on UI/text content).
const QUALITY_HIGH: u8 = 90;
const QUALITY_MED: u8 = 75;
const QUALITY_LOW: u8 = 60;

/// Tile side length in pixels. Industry-standard 64 px sweet spot — smaller
/// dirty-area packets, fewer SCTP fragmentations, finer change resolution.
/// The 4× tile-count bloat on full I-frames is acceptable because I-frames
/// happen only on session start / quality change / new viewer.
pub const TILE_SIZE: u32 = 64;

/// Global thumbnail dimensions for the two-tier hash short-circuit. Picked at
/// roughly the standard 16:9 aspect; xxh3 over ~5 KB is single-digit µs on
/// modern cores so the cost is dwarfed by the win on idle frames.
const THUMB_W: u32 = 96;
const THUMB_H: u32 = 54;

/// Idle ceiling for the capture loop's adaptive backoff. Eyes can detect a
/// mouse-jiggle wake at ~400 ms; any longer feels stuck.
pub const IDLE_INTERVAL_MAX_MS: u64 = 400;

/// Capture quality / resolution tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    High,
    Med,
    Low,
}

impl QualityTier {
    /// JPEG quality value for this tier.
    pub fn jpeg_quality(self) -> u8 {
        match self {
            QualityTier::High => QUALITY_HIGH,
            QualityTier::Med => QUALITY_MED,
            QualityTier::Low => QUALITY_LOW,
        }
    }

    /// Chroma subsampling per tier. High/Med keep 4:4:4 for crisp text on the
    /// retina path; Low drops to 4:2:0 (Sub2x2) to halve bandwidth on cellular.
    fn subsamp(self) -> Subsamp {
        match self {
            QualityTier::High | QualityTier::Med => Subsamp::None,
            QualityTier::Low => Subsamp::Sub2x2,
        }
    }

    /// Numerator of the tier resolution-scale fraction (denominator = 4).
    /// High = 4/4 (no tier reduction), Med = 3/4, Low = 2/4. HiDPI downscale
    /// is applied separately in `quality_resize` — this fraction is relative
    /// to the already-logical output.
    fn scale_num(self) -> u32 {
        match self {
            QualityTier::High => 4,
            QualityTier::Med => 3,
            QualityTier::Low => 2,
        }
    }
}

/// A single tile encoded as JPEG bytes, with grid position.
#[derive(Debug, Clone)]
pub struct EncodedTile {
    /// Tile column index (0-based, in tile units, not pixels).
    pub x: u16,
    /// Tile row index (0-based, in tile units, not pixels).
    pub y: u16,
    /// JPEG-encoded bytes for this tile.
    pub jpeg: Bytes,
}

/// Pixel byte order for an in-flight tile. xcap delivers RGBA; SCK delivers
/// BGRA. turbojpeg supports both natively so we just thread the discriminant
/// to the encoder instead of paying for a software channel swap per tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilePixelFormat {
    Rgba,
    Bgra,
}

/// One changed tile: grid coordinates plus packed pixel bytes ready for JPEG
/// encode (no per-pixel iteration needed by the encoder).
pub struct ChangedTile {
    pub col: u16,
    pub row: u16,
    pub w: u32,
    pub h: u32,
    /// Packed pixels, 4 bytes/pixel in the format declared by `format`.
    pub pixels: Vec<u8>,
    pub format: TilePixelFormat,
}

/// Output of one capture-encode cycle: changed tiles + timestamp.
#[derive(Debug)]
pub struct FrameOutput {
    /// Changed tiles (empty → idle frame, not emitted by CaptureLoop).
    pub tiles: Vec<EncodedTile>,
    /// Unix timestamp in milliseconds when this frame was captured.
    pub frame_ts: u64,
}

/// Resize `img` by composing HiDPI normalisation × tier scale.
///
/// xcap returns physical pixels (e.g. 3024×1964 on 2× retina), while clients
/// expect logical coordinates by default. On a 2× retina MBP at High tier the
/// naive 100% path yields a 24×16 tile grid (384 tiles); downscaling to logical
/// first gives 12×8 (96 tiles — 4× fewer tiles and pixels to encode).
///
/// When `hidpi_mode` is `true` the logical normalisation is skipped — the
/// encoder keeps physical-resolution input (modulated by tier only) so text
/// glyphs are rendered from native pixels rather than a downsampled
/// approximation. Costs ~4× encoder load on 2× retina; bitrate is bumped
/// by the caller to compensate. Phase 04 user toggle.
///
/// `scale_factor` is the display's physical-to-logical ratio from xcap
/// (`Monitor::scale_factor()`) or the objc2 backingScaleFactor fallback.
/// Returns the input unchanged at 1× logical + High tier (fast path).
pub fn quality_resize(
    img: RgbaImage,
    tier: QualityTier,
    scale_factor: f32,
    hidpi_mode: bool,
) -> RgbaImage {
    let hidpi_norm = if hidpi_mode { 1.0 } else { 1.0_f32 / scale_factor.max(1.0) };
    let tier_f = tier.scale_num() as f32 / 4.0;
    let effective = hidpi_norm * tier_f;

    if (effective - 1.0).abs() < 0.01 {
        return img;
    }

    let w = ((img.width() as f32 * effective) as u32).max(1);
    let h = ((img.height() as f32 * effective) as u32).max(1);
    fir_resize_rgba8(&img, w, h)
}

/// SIMD downscale via `fast_image_resize`. Roughly 4–5× faster than
/// `image::imageops::resize(_, FilterType::Triangle)` on Apple Silicon
/// (AArch64 NEON) and AVX2 on x86_64. Bilinear filter chosen because the
/// frame is rarely the user's focus pixel-for-pixel — H.264 + JPEG quality
/// dominates perceived sharpness, so paying for Lanczos here is wasted.
/// Falls back to image::imageops on any internal error so a one-frame hiccup
/// can't break the session.
fn fir_resize_rgba8(src: &RgbaImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let (sw, sh) = (src.width(), src.height());
    let src_fir = match FirImage::from_vec_u8(sw, sh, src.as_raw().clone(), PixelType::U8x4) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(error = %e, "fir_resize: src wrap failed, falling back");
            return image::imageops::resize(src, dst_w, dst_h, image::imageops::FilterType::Triangle);
        }
    };
    let mut dst_fir = FirImage::new(dst_w, dst_h, PixelType::U8x4);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilter::Bilinear));
    let mut resizer = Resizer::new();
    if let Err(e) = resizer.resize(&src_fir, &mut dst_fir, &opts) {
        tracing::warn!(error = %e, "fir_resize: resize failed, falling back");
        return image::imageops::resize(src, dst_w, dst_h, image::imageops::FilterType::Triangle);
    }
    RgbaImage::from_raw(dst_w, dst_h, dst_fir.into_vec()).unwrap_or_else(|| {
        // unreachable: from_vec_u8 above already validated 4 bytes/pixel
        image::imageops::resize(src, dst_w, dst_h, image::imageops::FilterType::Triangle)
    })
}

/// Compute the resized output dimensions for a (logical_w, logical_h) monitor
/// at a given tier and HiDPI mode. Mirrors `quality_resize` math without
/// capturing a frame. Used by the session runner to pre-emit `capabilities`
/// and to size the SCK output stream.
///
/// `scale_factor` is only consulted when `hidpi_mode` is true — it's the
/// physical/logical ratio used to expand the logical extent back to physical
/// before applying tier reduction.
pub fn resize_dims(
    logical_w: u32,
    logical_h: u32,
    tier: QualityTier,
    scale_factor: f32,
    hidpi_mode: bool,
) -> (u32, u32) {
    let num = tier.scale_num();
    let (base_w, base_h) = if hidpi_mode {
        let s = scale_factor.max(1.0);
        (
            ((logical_w as f32) * s).round() as u32,
            ((logical_h as f32) * s).round() as u32,
        )
    } else {
        (logical_w, logical_h)
    };
    let w = (base_w * num / 4).max(1);
    let h = (base_h * num / 4).max(1);
    (w, h)
}

/// Hash a thumbnail of `img` for the global short-circuit. Resized via
/// `fast_image_resize` to a fixed ~96×54 RGBA, then xxh3'd. ~5 KB hashed per
/// frame ≈ < 1 µs on Apple Silicon — well below the savings on idle frames.
pub fn global_thumbnail_hash(img: &RgbaImage) -> u64 {
    let thumb = fir_resize_rgba8(img, THUMB_W, THUMB_H);
    xxh3_64(thumb.as_raw())
}

/// Format-agnostic thumbnail hash. Treats the input as opaque 4-bytes/pixel
/// data — the channel order doesn't matter for change detection because
/// xxh3 is byte-keyed and we never compare hashes across formats inside a
/// session (TileDiff state resets on backend switch).
fn global_thumbnail_hash_bytes(bytes: &[u8], w: u32, h: u32) -> u64 {
    use fast_image_resize::images::Image as FirImage;
    let src_fir = match FirImage::from_vec_u8(w, h, bytes.to_vec(), PixelType::U8x4) {
        Ok(i) => i,
        Err(_) => return xxh3_64(bytes),
    };
    let mut dst_fir = FirImage::new(THUMB_W, THUMB_H, PixelType::U8x4);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilter::Bilinear));
    let mut resizer = Resizer::new();
    if resizer.resize(&src_fir, &mut dst_fir, &opts).is_err() {
        return xxh3_64(bytes);
    }
    xxh3_64(dst_fir.buffer())
}

/// Tracks per-tile xxhash3 values from the previous frame to detect changes,
/// plus a global thumbnail hash for the idle-frame short-circuit.
pub struct TileDiff {
    prev_hashes: Vec<u64>,
    prev_global: Option<u64>,
    cols: u32,
    rows: u32,
}

impl TileDiff {
    /// Create a new TileDiff with no previous-frame state.
    pub fn new() -> Self {
        TileDiff {
            prev_hashes: Vec::new(),
            prev_global: None,
            cols: 0,
            rows: 0,
        }
    }

    /// Force the next `diff()` call to report every tile as changed.
    ///
    /// Equivalent to an H.264 IDR for the tile-diff pipeline — used when a
    /// new peer connects so the joining viewer always sees a complete frame,
    /// not deltas from state it never had.
    pub fn reset(&mut self) {
        // Zero the vec in place when possible so the next diff reports every
        // tile changed (real xxh3 values never collide with u64::MAX on the
        // payload sizes we encode). Clearing cols/rows would also work, but
        // this keeps the grid dimensions stable in case the caller checks.
        for h in &mut self.prev_hashes {
            *h = u64::MAX;
        }
        // Force the global short-circuit to miss next call too.
        self.prev_global = None;
    }

    /// Compare an RGBA image against the previous frame. Convenience wrapper
    /// for callers that already hold an `RgbaImage` (xcap path + tests).
    pub fn diff(&mut self, img: &RgbaImage) -> Vec<ChangedTile> {
        self.diff_bytes(img.as_raw(), img.width(), img.height(), TilePixelFormat::Rgba)
    }

    /// Compare a packed pixel buffer against the previous frame.
    ///
    /// Format-agnostic — xxh3 hashes raw bytes so RGBA and BGRA both work
    /// without conversion. The format is stamped on each emitted
    /// `ChangedTile` so the encoder can pick the right turbojpeg pixel
    /// format. Used by both the xcap RGBA path and the SCK BGRA path.
    pub fn diff_bytes(
        &mut self,
        bytes: &[u8],
        img_w: u32,
        img_h: u32,
        format: TilePixelFormat,
    ) -> Vec<ChangedTile> {
        debug_assert_eq!(bytes.len(), (img_w * img_h * 4) as usize);

        // Two-tier hash: cheap thumbnail check first. If the whole frame
        // looks identical at coarse resolution we can skip the per-tile loop
        // entirely. Mismatch → fall through; we update prev_global only after
        // the per-tile pass so a mid-update bail-out doesn't desync state.
        let global = global_thumbnail_hash_bytes(bytes, img_w, img_h);
        if let Some(prev) = self.prev_global
            && prev == global
            && !self.prev_hashes.is_empty()
        {
            // No first-frame edge case: prev_hashes is empty until the first
            // real diff has populated it.
            return Vec::new();
        }

        // Integer ceiling division: (n + d - 1) / d
        let cols = img_w.div_ceil(TILE_SIZE);
        let rows = img_h.div_ceil(TILE_SIZE);
        let total = (cols * rows) as usize;

        // Reset hash table on resolution change.
        if self.cols != cols || self.rows != rows {
            self.prev_hashes = vec![u64::MAX; total]; // MAX never matches a real hash.
            self.cols = cols;
            self.rows = rows;
        }

        let src_stride = (img_w * 4) as usize;
        let mut tile_buf: Vec<u8> = Vec::with_capacity((TILE_SIZE * TILE_SIZE * 4) as usize);
        let mut changed: Vec<ChangedTile> = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let px = col * TILE_SIZE;
                let py = row * TILE_SIZE;
                let tw = (img_w - px).min(TILE_SIZE);
                let th = (img_h - py).min(TILE_SIZE);

                // Pack tile bytes by memcpy'ing each row slice — drops the
                // per-pixel walk that `view().to_image()` did via
                // `ImageBuffer::pixels()`.
                let row_bytes = (tw * 4) as usize;
                tile_buf.clear();
                tile_buf.reserve(row_bytes * th as usize);
                for ty in 0..th as usize {
                    let y = py as usize + ty;
                    let start = y * src_stride + (px * 4) as usize;
                    tile_buf.extend_from_slice(&bytes[start..start + row_bytes]);
                }

                let hash = xxh3_64(&tile_buf);
                let idx = (row * cols + col) as usize;
                if hash != self.prev_hashes[idx] {
                    self.prev_hashes[idx] = hash;
                    // Move the packed buffer to the changed list and reset
                    // the working buffer. capacity restored on next iteration.
                    let owned = std::mem::take(&mut tile_buf);
                    tile_buf = Vec::with_capacity((TILE_SIZE * TILE_SIZE * 4) as usize);
                    changed.push(ChangedTile {
                        col: col as u16,
                        row: row as u16,
                        w: tw,
                        h: th,
                        pixels: owned,
                        format,
                    });
                }
            }
        }

        // Update the global hash *after* the per-tile pass so prev_hashes and
        // prev_global stay consistent.
        self.prev_global = Some(global);
        changed
    }
}

impl Default for TileDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Encodes changed tiles to JPEG using libjpeg-turbo.
pub struct TileEncoder;

impl TileEncoder {
    /// Encode a list of changed tiles to JPEG bytes, in parallel via rayon.
    ///
    /// Each rayon worker constructs a fresh `turbojpeg::Compressor` —
    /// `Compressor` is `!Send` in its `BorrowedCompressor` form and the C
    /// handle is per-thread. Construction cost is single-digit µs so this is
    /// cheap relative to the encode itself. Input order is preserved by
    /// `into_par_iter().collect()` so callers observing tile ordering remain
    /// correct.
    pub fn encode(tiles: Vec<ChangedTile>, tier: QualityTier) -> Vec<EncodedTile> {
        let quality = tier.jpeg_quality();
        let subsamp = tier.subsamp();
        tiles
            .into_par_iter()
            .filter_map(|tile| {
                let jpeg = encode_tile_jpeg(
                    &tile.pixels,
                    tile.w,
                    tile.h,
                    quality,
                    subsamp,
                    tile.format,
                )
                .ok()?;
                Some(EncodedTile { x: tile.col, y: tile.row, jpeg })
            })
            .collect()
    }

    /// Run the full resize → diff → encode pipeline for one raw frame.
    ///
    /// Returns a `FrameOutput`. `tiles` will be empty for idle frames.
    pub fn process_frame(
        raw: RgbaImage,
        tier: QualityTier,
        scale_factor: f32,
        hidpi_mode: bool,
        diff: &mut TileDiff,
    ) -> FrameOutput {
        let frame_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let resized = quality_resize(raw, tier, scale_factor, hidpi_mode);
        let changed = diff.diff(&resized);
        let tiles = TileEncoder::encode(changed, tier);

        FrameOutput { tiles, frame_ts }
    }

    /// SCK fast path: BGRA bytes already pre-sized at tier dims, no software
    /// resize needed. The encoder picks `TjPixel::BGRA` from the per-tile
    /// format stamp set by `diff_bytes`.
    pub fn process_frame_bgra(
        bytes: &[u8],
        width: u32,
        height: u32,
        tier: QualityTier,
        diff: &mut TileDiff,
    ) -> FrameOutput {
        let frame_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let changed = diff.diff_bytes(bytes, width, height, TilePixelFormat::Bgra);
        let tiles = TileEncoder::encode(changed, tier);
        FrameOutput { tiles, frame_ts }
    }
}

/// Encode a packed pixel buffer as JPEG via libjpeg-turbo.
///
/// `pixels` is `width * height * 4` bytes in either RGBA or BGRA order
/// (alpha ignored by the JPEG colour space). turbojpeg accepts both natively
/// and uses NEON / AVX2 SIMD internally — no software channel swap.
fn encode_tile_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    subsamp: Subsamp,
    format: TilePixelFormat,
) -> anyhow::Result<Bytes> {
    debug_assert_eq!(pixels.len(), (width * height * 4) as usize);

    let img = TjImage {
        pixels,
        width: width as usize,
        pitch: (width * 4) as usize,
        height: height as usize,
        format: match format {
            TilePixelFormat::Rgba => TjPixel::RGBA,
            TilePixelFormat::Bgra => TjPixel::BGRA,
        },
    };

    let mut compressor =
        Compressor::new().map_err(|e| anyhow::anyhow!("turbojpeg compressor: {e}"))?;
    compressor
        .set_quality(quality as i32)
        .map_err(|e| anyhow::anyhow!("turbojpeg quality: {e}"))?;
    compressor
        .set_subsamp(subsamp)
        .map_err(|e| anyhow::anyhow!("turbojpeg subsamp: {e}"))?;

    let jpeg_vec = compressor
        .compress_to_vec(img)
        .map_err(|e| anyhow::anyhow!("turbojpeg encode: {e}"))?;

    Ok(Bytes::from(jpeg_vec))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid_rgba(w: u32, h: u32, color: Rgba<u8>) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for px in img.pixels_mut() {
            *px = color;
        }
        img
    }

    /// TileDiff must report exactly the tiles that changed between frames.
    /// At TILE_SIZE = 64, a 256×256 frame is a 4×4 grid (16 tiles); modifying
    /// the upper-right 128×128 quadrant covers cols 2..=3, rows 0..=1 → 4 tiles.
    #[test]
    fn tile_diff_detects_single_change() {
        let base = solid_rgba(256, 256, Rgba([100, 100, 100, 255]));
        let mut diff = TileDiff::new();

        // First frame: all 16 tiles reported as changed.
        let first = diff.diff(&base);
        assert_eq!(first.len(), 16, "first frame must return all 16 tiles");

        // Second frame: identical to first — no changes.
        let none = diff.diff(&base);
        assert!(none.is_empty(), "identical frame must yield zero changed tiles");

        // Modify the upper-right 128×128 region: cols 2-3, rows 0-1 at 64-px.
        let mut modified = base.clone();
        for x in 128..256u32 {
            for y in 0..128u32 {
                modified.put_pixel(x, y, Rgba([200, 50, 50, 255]));
            }
        }

        let changed = diff.diff(&modified);
        assert_eq!(changed.len(), 4, "exactly four 64-px tiles cover the modified quadrant");
        for tile in &changed {
            assert!(tile.col == 2 || tile.col == 3, "col must be 2 or 3");
            assert!(tile.row == 0 || tile.row == 1, "row must be 0 or 1");
        }
    }

    /// After `reset()` every tile must re-register as changed on the next diff.
    #[test]
    fn tile_diff_reset_forces_full_frame() {
        let base = solid_rgba(256, 256, Rgba([100, 100, 100, 255]));
        let mut diff = TileDiff::new();

        let _ = diff.diff(&base);
        assert!(diff.diff(&base).is_empty());

        diff.reset();
        let all_again = diff.diff(&base);
        assert_eq!(all_again.len(), 16, "reset must force every tile changed");
    }

    /// Two-tier hash short-circuit: identical frames bail out before the
    /// per-tile pass. Verified by checking the global hash matches on the
    /// second call (semantic check; the early-return path is the same code
    /// that produces an empty Vec, so the assertion is on `is_empty()`).
    #[test]
    fn global_thumbnail_short_circuits_idle() {
        let base = solid_rgba(512, 512, Rgba([42, 42, 42, 255]));
        let mut diff = TileDiff::new();

        // First call seeds prev_global.
        let first = diff.diff(&base);
        assert!(!first.is_empty(), "first frame seeds the diff state");

        // Second call: thumbnail matches → early return. prev_hashes is intact.
        let idle = diff.diff(&base);
        assert!(idle.is_empty(), "idle thumbnail short-circuit must yield empty");
        // Confirm the short-circuit kept the state consistent so a real change
        // afterwards still flows through normally.
        let mut moved = base.clone();
        for x in 0..32 {
            for y in 0..32 {
                moved.put_pixel(x, y, Rgba([200, 50, 50, 255]));
            }
        }
        let after = diff.diff(&moved);
        assert!(!after.is_empty(), "real change after idle must still be detected");
    }

    /// `resize_dims(logical, tier, scale_factor, hidpi)` must agree with
    /// `quality_resize(physical_img, tier, scale_factor, hidpi)` for every
    /// (tier, hidpi) combination — otherwise the `Capabilities` message
    /// disagrees with the encoder grid and the client canvas misaligns.
    /// Locks the math contract across any future xcap or tier changes.
    #[test]
    fn resize_dims_agrees_with_quality_resize_on_retina() {
        // 2× retina MBP physical 3024×1964, logical 1512×982.
        let logical_w = 1512;
        let logical_h = 982;
        let physical_w = logical_w * 2;
        let physical_h = logical_h * 2;
        let scale_factor = 2.0_f32;

        for tier in [QualityTier::High, QualityTier::Med, QualityTier::Low] {
            for hidpi in [false, true] {
                let physical = solid_rgba(physical_w, physical_h, Rgba([0, 0, 0, 255]));
                let emitted = quality_resize(physical, tier, scale_factor, hidpi);
                let predicted = resize_dims(logical_w, logical_h, tier, scale_factor, hidpi);
                assert_eq!(
                    (emitted.width(), emitted.height()),
                    predicted,
                    "tier {tier:?} hidpi={hidpi}: resize_dims predicted {predicted:?} but quality_resize emitted {}×{}",
                    emitted.width(),
                    emitted.height()
                );
            }
        }
    }

    /// `quality_resize` composes HiDPI normalisation with tier scale.
    #[test]
    fn quality_resize_composes_hidpi_and_tier() {
        // 2× retina physical → logical at High: 200×200 → 100×100
        let img = solid_rgba(200, 200, Rgba([0, 0, 0, 255]));
        let out = quality_resize(img.clone(), QualityTier::High, 2.0, false);
        assert_eq!(out.width(), 100);
        assert_eq!(out.height(), 100);

        // 2× retina + Low tier (0.5): 200 × (1/2) × (2/4) = 50
        let out_low = quality_resize(img.clone(), QualityTier::Low, 2.0, false);
        assert_eq!(out_low.width(), 50);
        assert_eq!(out_low.height(), 50);

        // 1× scale + High: no-op fast path (same width/height)
        let out_noop = quality_resize(img, QualityTier::High, 1.0, false);
        assert_eq!(out_noop.width(), 200);
    }

    /// HiDPI mode skips the 1/scale_factor logical-normalisation step. On
    /// 2× retina input the encoder receives the full physical resolution
    /// (modulated only by tier), preserving native pixel detail for text.
    #[test]
    fn resize_dims_with_hidpi_skips_logical_normalization() {
        let img = solid_rgba(200, 200, Rgba([0, 0, 0, 255]));

        // hidpi=true + High: tier_f = 1.0, hidpi_norm = 1.0 → identity (200×200).
        let out_high = quality_resize(img.clone(), QualityTier::High, 2.0, true);
        assert_eq!(
            (out_high.width(), out_high.height()),
            (200, 200),
            "HiDPI High must keep physical resolution"
        );

        // hidpi=true + Med: tier_f = 0.75, hidpi_norm = 1.0 → 150×150.
        let out_med = quality_resize(img.clone(), QualityTier::Med, 2.0, true);
        assert_eq!((out_med.width(), out_med.height()), (150, 150));

        // hidpi=true + Low: tier_f = 0.5, hidpi_norm = 1.0 → 100×100.
        let out_low = quality_resize(img, QualityTier::Low, 2.0, true);
        assert_eq!((out_low.width(), out_low.height()), (100, 100));

        // resize_dims must agree with the above for the same logical input.
        // logical = physical / scale = 100. HiDPI re-expands to 200, then tier.
        assert_eq!(resize_dims(100, 100, QualityTier::High, 2.0, true), (200, 200));
        assert_eq!(resize_dims(100, 100, QualityTier::Med, 2.0, true), (150, 150));
        assert_eq!(resize_dims(100, 100, QualityTier::Low, 2.0, true), (100, 100));
    }

    /// Parallel encode preserves input ordering (rayon docs contract).
    #[test]
    fn encode_preserves_tile_order() {
        let tiles: Vec<ChangedTile> = (0..8u16)
            .map(|i| {
                let mut rgba = vec![0u8; 16 * 16 * 4];
                for px in rgba.chunks_exact_mut(4) {
                    px[0] = (i as u8) * 10;
                    px[3] = 255;
                }
                ChangedTile {
                    col: i,
                    row: 0,
                    w: 16,
                    h: 16,
                    pixels: rgba,
                    format: TilePixelFormat::Rgba,
                }
            })
            .collect();
        let out = TileEncoder::encode(tiles, QualityTier::Low);
        assert_eq!(out.len(), 8);
        for (i, t) in out.iter().enumerate() {
            assert_eq!(t.x, i as u16, "tile x must equal input index");
        }
    }

    /// Encode a solid-red 128×128 tile to JPEG and decode it back.
    /// Mean absolute error per channel must be < 10.
    #[test]
    fn encode_decode_roundtrip() {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 255; // R
            px[3] = 255; // A
        }
        let jpeg = encode_tile_jpeg(
            &rgba,
            128,
            128,
            QUALITY_HIGH,
            Subsamp::None,
            TilePixelFormat::Rgba,
        )
        .expect("encode must succeed");

        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("decode must succeed")
            .to_rgba8();

        assert_eq!(decoded.width(), 128);
        assert_eq!(decoded.height(), 128);

        // Compute mean absolute error across R, G, B channels.
        let total_pixels = (128 * 128) as f64;
        let mut sum_delta = [0u64; 3];
        for (orig, dec) in rgba.chunks_exact(4).zip(decoded.pixels()) {
            for ch in 0..3 {
                sum_delta[ch] += (orig[ch] as i32 - dec[ch] as i32).unsigned_abs() as u64;
            }
        }
        for ch in 0..3 {
            let mean = sum_delta[ch] as f64 / total_pixels;
            assert!(
                mean < 10.0,
                "channel {ch} mean delta {mean:.2} exceeds threshold of 10"
            );
        }
    }

    /// BGRA encode round-trips with the same fidelity as the RGBA path —
    /// turbojpeg's BGRA pixel format is supported natively, no software swap.
    #[test]
    fn bgra_encode_roundtrips_within_threshold() {
        let mut bgra = vec![0u8; 128 * 128 * 4];
        for px in bgra.chunks_exact_mut(4) {
            px[2] = 255; // R (in B-G-R-A order, R is third)
            px[3] = 255; // A
        }
        let jpeg = encode_tile_jpeg(
            &bgra,
            128,
            128,
            QUALITY_HIGH,
            Subsamp::None,
            TilePixelFormat::Bgra,
        )
        .expect("encode must succeed");

        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("decode must succeed")
            .to_rgba8();

        // Decoded image's R channel should match the input's R (which lived
        // in BGRA byte 2). Mean delta < 10 mirrors the RGBA test.
        let total = (128 * 128) as f64;
        let mut delta_r = 0u64;
        for dec in decoded.pixels() {
            delta_r += (255 - dec[0] as i32).unsigned_abs() as u64;
        }
        let mean_r = delta_r as f64 / total;
        assert!(mean_r < 10.0, "BGRA → JPEG → RGBA R-channel mean delta {mean_r:.2}");
    }

    /// `diff_bytes` and `diff(&RgbaImage)` produce byte-identical change lists
    /// for the same input. Locks the format-agnostic refactor.
    #[test]
    fn diff_bytes_matches_diff_image() {
        let img = solid_rgba(256, 256, Rgba([100, 100, 100, 255]));
        let mut da = TileDiff::new();
        let mut db = TileDiff::new();
        let a = da.diff(&img);
        let b = db.diff_bytes(img.as_raw(), 256, 256, TilePixelFormat::Rgba);
        assert_eq!(a.len(), b.len());
        for (ta, tb) in a.iter().zip(b.iter()) {
            assert_eq!((ta.col, ta.row, ta.w, ta.h), (tb.col, tb.row, tb.w, tb.h));
            assert_eq!(ta.pixels, tb.pixels);
        }
    }

    /// FPS cap test: run CaptureLoop for ~1 second at High quality.
    /// Expected: 28–32 frames received.
    ///
    /// Requires a real display — ignored in headless CI. Run locally with:
    ///   cargo test -p desktop fps_cap_respects_30 -- --ignored
    #[test]
    #[ignore = "requires display; run locally only"]
    fn fps_cap_respects_30() {
        use crate::capture::CaptureLoop;
        use std::time::Duration;
        use tokio::sync::mpsc;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel::<FrameOutput>(64);
            let tier = QualityTier::High;

            // Spawn blocking capture loop with default 1× scale (CI).
            tokio::task::spawn_blocking(move || {
                CaptureLoop::run(tier, tx, 1.0, false, None, None)
            });

            // Collect frames for 1 second.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
            let mut count = 0usize;
            while tokio::time::Instant::now() < deadline {
                let timeout =
                    tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
                if let Ok(Some(_)) = timeout {
                    count += 1;
                }
            }
            // Drop rx to signal loop to stop.
            drop(rx);

            assert!(
                (28..=32).contains(&count),
                "expected 28–32 frames in 1s at High quality, got {count}"
            );
        });
    }
}
