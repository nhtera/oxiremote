/// Tile-diff encoder: resize → xxhash tile comparison → mozjpeg JPEG encode.
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};
use xxhash_rust::xxh3::xxh3_64;

/// JPEG quality per tier (0–100).
const QUALITY_HIGH: u8 = 85;
const QUALITY_MED: u8 = 75;
const QUALITY_LOW: u8 = 60;

/// Tile side length in pixels.
const TILE_SIZE: u32 = 128;

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

    /// Numerator of the resolution scale fraction (denominator = 4).
    fn scale_num(self) -> u32 {
        match self {
            QualityTier::High => 4, // 100%
            QualityTier::Med => 3,  // 75%
            QualityTier::Low => 2,  // 50%
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

/// Resize `img` according to `tier` using a Triangle (bilinear) filter.
/// High=100%, Med=75%, Low=50% of native resolution.
pub fn quality_resize(img: RgbaImage, tier: QualityTier) -> RgbaImage {
    let scale = tier.scale_num();
    if scale == 4 {
        return img; // No resize needed for High tier.
    }
    let w = img.width() * scale / 4;
    let h = img.height() * scale / 4;
    // Clamp to at least 1×1 to avoid a panic on zero-size images.
    let w = w.max(1);
    let h = h.max(1);
    image::imageops::resize(&img, w, h, FilterType::Triangle)
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
    /// Encode a list of changed tiles to JPEG bytes.
    ///
    /// A fresh `mozjpeg::Compress` is created per tile — mozjpeg is not
    /// `Send`, so do not share a compressor across threads or closures.
    pub fn encode(tiles: Vec<(u16, u16, RgbaImage)>, quality: u8) -> Vec<EncodedTile> {
        tiles
            .into_iter()
            .filter_map(|(x, y, tile)| {
                let jpeg = encode_tile_jpeg(&tile, quality).ok()?;
                Some(EncodedTile { x, y, jpeg })
            })
            .collect()
    }

    /// Run the full resize → diff → encode pipeline for one raw frame.
    ///
    /// Returns a `FrameOutput`. `tiles` will be empty for idle frames.
    pub fn process_frame(raw: RgbaImage, tier: QualityTier, diff: &mut TileDiff) -> FrameOutput {
        let frame_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let resized = quality_resize(raw, tier);
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

            // Spawn blocking capture loop.
            tokio::task::spawn_blocking(move || CaptureLoop::run(tier, tx));

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
