/// Tile-diff encoder: resize → xxhash tile comparison → mozjpeg JPEG encode.
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};
use rayon::prelude::*;
use xxhash_rust::xxh3::xxh3_64;

/// JPEG quality per tier (0–100).
const QUALITY_HIGH: u8 = 85;
const QUALITY_MED: u8 = 75;
const QUALITY_LOW: u8 = 60;

/// Tile side length in pixels.
pub const TILE_SIZE: u32 = 128;

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
/// expect logical coordinates. On a 2× retina MBP at High tier the naive
/// 100% path yields a 24×16 tile grid (384 tiles); downscaling to logical
/// first gives 12×8 (96 tiles — 4× fewer tiles and pixels to encode).
///
/// `scale_factor` is the display's physical-to-logical ratio from xcap
/// (`Monitor::scale_factor()`) or the objc2 backingScaleFactor fallback.
/// Returns the input unchanged at 1× logical + High tier (fast path).
pub fn quality_resize(img: RgbaImage, tier: QualityTier, scale_factor: f32) -> RgbaImage {
    let hidpi = 1.0_f32 / scale_factor.max(1.0);
    let tier_f = tier.scale_num() as f32 / 4.0;
    let effective = hidpi * tier_f;

    if (effective - 1.0).abs() < 0.01 {
        return img;
    }

    let w = ((img.width() as f32 * effective) as u32).max(1);
    let h = ((img.height() as f32 * effective) as u32).max(1);
    image::imageops::resize(&img, w, h, FilterType::Triangle)
}

/// Compute the resized output dimensions for a (logical_w, logical_h) monitor
/// at a given tier. Mirrors `quality_resize` math without capturing a frame.
/// Used by the session runner to pre-emit `capabilities` to clients.
pub fn resize_dims(logical_w: u32, logical_h: u32, tier: QualityTier) -> (u32, u32) {
    let num = tier.scale_num();
    let w = (logical_w * num / 4).max(1);
    let h = (logical_h * num / 4).max(1);
    (w, h)
}

/// Tracks per-tile xxhash3 values from the previous frame to detect changes.
pub struct TileDiff {
    prev_hashes: Vec<u64>,
    cols: u32,
    rows: u32,
}

impl TileDiff {
    /// Create a new TileDiff with no previous-frame state.
    pub fn new() -> Self {
        TileDiff {
            prev_hashes: Vec::new(),
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
    }

    /// Compare `img` against the previous frame.
    ///
    /// Returns a list of `(tile_col, tile_row, tile_rgba)` for every tile
    /// whose xxhash3 differs from the stored hash. On the first call (no
    /// previous state), all tiles are returned as changed.
    pub fn diff(&mut self, img: &RgbaImage) -> Vec<(u16, u16, RgbaImage)> {
        let img_w = img.width();
        let img_h = img.height();

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

        let mut changed = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let px = col * TILE_SIZE;
                let py = row * TILE_SIZE;
                let tw = (img_w - px).min(TILE_SIZE);
                let th = (img_h - py).min(TILE_SIZE);

                // Extract tile RGBA bytes.
                let tile_img = img.view(px, py, tw, th).to_image();
                let tile_bytes = tile_img.as_raw();
                let hash = xxh3_64(tile_bytes);

                let idx = (row * cols + col) as usize;
                if hash != self.prev_hashes[idx] {
                    self.prev_hashes[idx] = hash;
                    changed.push((col as u16, row as u16, tile_img));
                }
            }
        }

        changed
    }
}

impl Default for TileDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Encodes changed tiles to JPEG using mozjpeg.
pub struct TileEncoder;

impl TileEncoder {
    /// Encode a list of changed tiles to JPEG bytes, in parallel via rayon.
    ///
    /// `mozjpeg::Compress` is `!Send`, but each call constructs a fresh
    /// instance — rayon workers never share one, so the parallel path is
    /// safe. Input order is preserved by `into_par_iter().collect()` so
    /// existing callers observing tile ordering remain correct.
    pub fn encode(tiles: Vec<(u16, u16, RgbaImage)>, quality: u8) -> Vec<EncodedTile> {
        tiles
            .into_par_iter()
            .filter_map(|(x, y, tile)| {
                let jpeg = encode_tile_jpeg(&tile, quality).ok()?;
                Some(EncodedTile { x, y, jpeg })
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
        diff: &mut TileDiff,
    ) -> FrameOutput {
        let frame_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let resized = quality_resize(raw, tier, scale_factor);
        let changed_tiles = diff.diff(&resized);
        let tiles = TileEncoder::encode(changed_tiles, tier.jpeg_quality());

        FrameOutput { tiles, frame_ts }
    }
}

/// Encode a single RGBA tile as JPEG bytes via mozjpeg.
fn encode_tile_jpeg(tile: &RgbaImage, quality: u8) -> anyhow::Result<Bytes> {
    let width = tile.width() as usize;
    let height = tile.height() as usize;

    // mozjpeg encodes RGB, not RGBA — strip the alpha channel.
    let rgb_bytes: Vec<u8> = tile
        .pixels()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();

    // mozjpeg 0.10 API: Compress takes a writer; Vec<u8> acts as the sink.
    // Compress is NOT Send — create fresh per call (never share across threads).
    let mut compress = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
    compress.set_size(width, height);
    compress.set_quality(quality as f32);

    let buf: Vec<u8> = Vec::new();
    let mut started = compress
        .start_compress(buf)
        .map_err(|e| anyhow::anyhow!("mozjpeg start_compress: {e}"))?;

    // Write scan lines row by row.
    let row_stride = width * 3;
    for row in 0..height {
        let start = row * row_stride;
        let end = start + row_stride;
        started
            .write_scanlines(&rgb_bytes[start..end])
            .map_err(|_| anyhow::anyhow!("mozjpeg write_scanlines failed at row {row}"))?;
    }

    let jpeg_vec = started
        .finish()
        .map_err(|e| anyhow::anyhow!("mozjpeg finish: {e}"))?;

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

    /// TileDiff must report exactly the one tile that changed between frames.
    #[test]
    fn tile_diff_detects_single_change() {
        // 256×256 image → 2×2 tiles (each 128×128).
        let base = solid_rgba(256, 256, Rgba([100, 100, 100, 255]));
        let mut diff = TileDiff::new();

        // First frame: all tiles reported as changed.
        let first = diff.diff(&base);
        assert_eq!(first.len(), 4, "first frame must return all 4 tiles");

        // Second frame: identical to first — no changes.
        let none = diff.diff(&base);
        assert!(none.is_empty(), "identical frame must yield zero changed tiles");

        // Modify tile at (col=1, row=0): pixel at x=128..255, y=0..127.
        let mut modified = base.clone();
        for x in 128..256u32 {
            for y in 0..128u32 {
                modified.put_pixel(x, y, Rgba([200, 50, 50, 255]));
            }
        }

        let changed = diff.diff(&modified);
        assert_eq!(changed.len(), 1, "only one tile should change");
        let (cx, cy, _) = &changed[0];
        assert_eq!(*cx, 1, "changed tile col must be 1");
        assert_eq!(*cy, 0, "changed tile row must be 0");
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
        assert_eq!(all_again.len(), 4, "reset must force every tile changed");
    }

    /// `resize_dims(logical, tier)` must produce the same output size that
    /// `quality_resize(physical_img, tier, scale_factor)` actually emits —
    /// otherwise the `Capabilities` message disagrees with the tile grid
    /// and the client canvas misaligns. Locks the math contract across
    /// any future xcap or tier scale-factor changes.
    #[test]
    fn resize_dims_agrees_with_quality_resize_on_retina() {
        // 2× retina MBP physical 3024×1964, logical 1512×982.
        let logical_w = 1512;
        let logical_h = 982;
        let physical_w = logical_w * 2;
        let physical_h = logical_h * 2;
        let scale_factor = 2.0_f32;

        for tier in [QualityTier::High, QualityTier::Med, QualityTier::Low] {
            let physical = solid_rgba(physical_w, physical_h, Rgba([0, 0, 0, 255]));
            let emitted = quality_resize(physical, tier, scale_factor);
            let predicted = resize_dims(logical_w, logical_h, tier);
            assert_eq!(
                (emitted.width(), emitted.height()),
                predicted,
                "tier {tier:?}: resize_dims predicted {predicted:?} but quality_resize emitted {}×{}",
                emitted.width(),
                emitted.height()
            );
        }
    }

    /// `quality_resize` composes HiDPI normalisation with tier scale.
    #[test]
    fn quality_resize_composes_hidpi_and_tier() {
        // 2× retina physical → logical at High: 200×200 → 100×100
        let img = solid_rgba(200, 200, Rgba([0, 0, 0, 255]));
        let out = quality_resize(img.clone(), QualityTier::High, 2.0);
        assert_eq!(out.width(), 100);
        assert_eq!(out.height(), 100);

        // 2× retina + Low tier (0.5): 200 × (1/2) × (2/4) = 50
        let out_low = quality_resize(img.clone(), QualityTier::Low, 2.0);
        assert_eq!(out_low.width(), 50);
        assert_eq!(out_low.height(), 50);

        // 1× scale + High: no-op fast path (same width/height)
        let out_noop = quality_resize(img, QualityTier::High, 1.0);
        assert_eq!(out_noop.width(), 200);
    }

    /// Parallel encode preserves input ordering (rayon docs contract).
    #[test]
    fn encode_preserves_tile_order() {
        let tiles: Vec<(u16, u16, RgbaImage)> = (0..8u16)
            .map(|i| (i, 0, solid_rgba(16, 16, Rgba([i as u8 * 10, 0, 0, 255]))))
            .collect();
        let out = TileEncoder::encode(tiles, QUALITY_LOW);
        assert_eq!(out.len(), 8);
        for (i, t) in out.iter().enumerate() {
            assert_eq!(t.x, i as u16, "tile x must equal input index");
        }
    }

    /// Encode a solid-red 128×128 tile to JPEG and decode it back.
    /// Mean absolute error per channel must be < 10.
    #[test]
    fn encode_decode_roundtrip() {
        let tile = solid_rgba(128, 128, Rgba([255, 0, 0, 255]));
        let jpeg = encode_tile_jpeg(&tile, QUALITY_HIGH).expect("encode must succeed");

        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("decode must succeed")
            .to_rgba8();

        assert_eq!(decoded.width(), 128);
        assert_eq!(decoded.height(), 128);

        // Compute mean absolute error across R, G, B channels.
        let total_pixels = (128 * 128) as f64;
        let mut sum_delta = [0u64; 3];
        for (orig, dec) in tile.pixels().zip(decoded.pixels()) {
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
            tokio::task::spawn_blocking(move || CaptureLoop::run(tier, tx, 1.0, None));

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
