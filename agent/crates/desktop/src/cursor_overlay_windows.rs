//! Windows OS cursor overlay for captured RGBA frames.
//!
//! xcap on Windows uses Windows.Graphics.Capture and calls
//! `SetIsCursorCaptureEnabled(false)` unconditionally — the OS cursor never
//! reaches the captured RGBA buffer. We compose it back in here by reading
//! the live cursor with `GetCursorInfo` + `GetIconInfo` + `GetDIBits` and
//! alpha-blending the decoded sprite onto the frame at the cursor position.
//!
//! Decoded sprites are cached by `HCURSOR` handle — Windows reuses the same
//! handle for a given shape, so most frames hit the cache and skip the GDI
//! round-trip. Cache is capped at 32 sprites; on overflow it fully clears.
//!
//! All calls are best-effort. Any failure (cursor hidden, GetIconInfo fails,
//! cursor off-monitor) skips compositing for that frame — a missing cursor
//! must never stall the capture loop.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use image::RgbaImage;

use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, CopyIcon, DestroyCursor, GetCursorInfo, GetIconInfo, HCURSOR,
    ICONINFO,
};

struct Sprite {
    /// Pre-decoded RGBA pixels, top-down, tightly packed.
    pixels: Vec<u8>,
    width: i32,
    height: i32,
    hotspot_x: i32,
    hotspot_y: i32,
}

static CACHE: OnceLock<Mutex<HashMap<isize, Sprite>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<isize, Sprite>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Composite the current OS cursor onto `rgba`.
///
/// `monitor_origin` is the captured monitor's top-left in virtual-screen
/// coords. `scale_factor` is the physical/logical pixel ratio. The agent
/// process is DPI-unaware (see `permissions.rs::normalize_to_logical`), so
/// `GetCursorInfo` and `monitor.x/y` both return LOGICAL coords — but
/// `xcap` captures the framebuffer at PHYSICAL pixels. Multiplying the
/// logical-local cursor position by `scale_factor` lands the sprite in
/// the right place on the physical-pixel buffer. At 125% scale the offset
/// without this fix is 20%; at 200% the cursor would land at half the
/// correct distance.
///
/// No-op when the cursor is hidden, off-monitor, or any GDI call fails.
pub fn compose(rgba: &mut RgbaImage, monitor_origin: (i32, i32), scale_factor: f32) {
    let info = unsafe {
        let mut ci: CURSORINFO = std::mem::zeroed();
        ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        if GetCursorInfo(&mut ci) == 0 {
            return;
        }
        // Skip when the cursor is hidden (flags == 0) or suppressed by the
        // OS (Windows 8+ touch input). Bitmask-test so future flag bits
        // don't break the check.
        if (ci.flags & CURSOR_SHOWING) == 0 || ci.hCursor.is_null() {
            return;
        }
        ci
    };

    let key = info.hCursor as isize;
    let scale = scale_factor.max(1.0);
    let logical_x = info.ptScreenPos.x - monitor_origin.0;
    let logical_y = info.ptScreenPos.y - monitor_origin.1;
    let pt_x = (logical_x as f32 * scale).round() as i32;
    let pt_y = (logical_y as f32 * scale).round() as i32;

    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    if !guard.contains_key(&key) {
        let Some(sprite) = (unsafe { decode_cursor(info.hCursor) }) else {
            return;
        };
        if guard.len() >= 32 {
            guard.clear();
        }
        guard.insert(key, sprite);
    }
    let sprite = guard.get(&key).expect("just inserted");
    blit(rgba, sprite, pt_x - sprite.hotspot_x, pt_y - sprite.hotspot_y);
}

unsafe fn decode_cursor(hcursor: HCURSOR) -> Option<Sprite> {
    // CopyIcon so the caller can free both bitmaps + the cursor without
    // touching the live OS cursor (`hcursor`).
    let copy = unsafe { CopyIcon(hcursor) };
    if copy.is_null() {
        return None;
    }
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetIconInfo(copy, &mut info) } == 0 {
        unsafe { DestroyCursor(copy) };
        return None;
    }

    let result = unsafe { decode_icon_info(&info) };

    // Always free the bitmaps + cursor copy, even on failure.
    if !info.hbmColor.is_null() {
        unsafe { DeleteObject(info.hbmColor) };
    }
    if !info.hbmMask.is_null() {
        unsafe { DeleteObject(info.hbmMask) };
    }
    unsafe { DestroyCursor(copy) };
    result
}

unsafe fn decode_icon_info(info: &ICONINFO) -> Option<Sprite> {
    let bmp_handle = if !info.hbmColor.is_null() {
        info.hbmColor
    } else {
        info.hbmMask
    };
    let mut bmp: BITMAP = unsafe { std::mem::zeroed() };
    let got = unsafe {
        GetObjectW(
            bmp_handle,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bmp as *mut _ as *mut _,
        )
    };
    if got == 0 || bmp.bmWidth <= 0 || bmp.bmHeight == 0 {
        return None;
    }

    let width = bmp.bmWidth;
    // Mono cursor: hbmMask is `width × (height * 2)` — top half AND, bottom half XOR.
    let height = if info.hbmColor.is_null() {
        bmp.bmHeight.abs() / 2
    } else {
        bmp.bmHeight.abs()
    };
    if width <= 0 || height <= 0 || width > 256 || height > 256 {
        return None;
    }

    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    if dc.is_null() {
        return None;
    }

    let mut sprite = Sprite {
        pixels: vec![0u8; (width * height * 4) as usize],
        width,
        height,
        hotspot_x: info.xHotspot as i32,
        hotspot_y: info.yHotspot as i32,
    };

    let ok = if !info.hbmColor.is_null() {
        unsafe { fill_color_sprite(dc, info, width, height, &mut sprite.pixels) }
    } else {
        unsafe { fill_mono_sprite(dc, info.hbmMask, width, height, &mut sprite.pixels) }
    };

    unsafe { DeleteDC(dc) };
    if ok { Some(sprite) } else { None }
}

/// 32-bpp colour cursor path. Read the color bitmap as BGRA; if it carries
/// no alpha, derive alpha from `hbmMask` (1-bpp AND mask, 0 = opaque).
unsafe fn fill_color_sprite(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    info: &ICONINFO,
    width: i32,
    height: i32,
    out: &mut [u8],
) -> bool {
    let mut bi = bitmap_info(width, -height);
    let mut buf = vec![0u8; (width * height * 4) as usize];
    let lines = unsafe {
        GetDIBits(
            dc,
            info.hbmColor,
            0,
            height as u32,
            buf.as_mut_ptr() as *mut _,
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    if lines <= 0 {
        return false;
    }

    let has_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
    if has_alpha {
        for (src, dst) in buf.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        return true;
    }

    // No alpha: read AND-mask, 0 = opaque pixel, 255 = transparent.
    let mut mask_bi = bitmap_info(width, -height);
    let mut mask_buf = vec![0u8; (width * height * 4) as usize];
    let mlines = unsafe {
        GetDIBits(
            dc,
            info.hbmMask,
            0,
            height as u32,
            mask_buf.as_mut_ptr() as *mut _,
            &mut mask_bi,
            DIB_RGB_COLORS,
        )
    };
    if mlines <= 0 {
        return false;
    }
    for (i, (src, dst)) in buf
        .chunks_exact(4)
        .zip(out.chunks_exact_mut(4))
        .enumerate()
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = if mask_buf[i * 4] == 0 { 255 } else { 0 };
    }
    true
}

/// Monochrome cursor path. `hbmMask` is `width × (height * 2)`:
/// top half = AND mask (0 = opaque), bottom half = XOR mask (1 = white).
unsafe fn fill_mono_sprite(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    hbm_mask: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    width: i32,
    height: i32,
    out: &mut [u8],
) -> bool {
    let total = height * 2;
    let mut bi = bitmap_info(width, -total);
    let mut buf = vec![0u8; (width * total * 4) as usize];
    let lines = unsafe {
        GetDIBits(
            dc,
            hbm_mask,
            0,
            total as u32,
            buf.as_mut_ptr() as *mut _,
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    if lines <= 0 {
        return false;
    }
    let row_bytes = (width * 4) as usize;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let and_idx = y * row_bytes + x * 4;
            let xor_idx = (y + height as usize) * row_bytes + x * 4;
            let and = buf[and_idx];
            let xor = buf[xor_idx];
            let dst_idx = (y * width as usize + x) * 4;
            // 4 cases by (AND, XOR): (0,0)=black, (0,1)=white, (1,0)=transparent,
            // (1,1)=invert dst. We collapse invert → transparent: rendering an
            // inverted-destination pixel needs the underlying frame, which
            // is fine to skip — the cursor still reads correctly.
            if and == 0 && xor == 0 {
                out[dst_idx] = 0;
                out[dst_idx + 1] = 0;
                out[dst_idx + 2] = 0;
                out[dst_idx + 3] = 255;
            } else if and == 0 {
                out[dst_idx] = 255;
                out[dst_idx + 1] = 255;
                out[dst_idx + 2] = 255;
                out[dst_idx + 3] = 255;
            } else {
                out[dst_idx + 3] = 0;
            }
        }
    }
    true
}

fn bitmap_info(width: i32, height_signed: i32) -> BITMAPINFO {
    let mut bi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: height_signed,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    bi
}

fn blit(rgba: &mut RgbaImage, sprite: &Sprite, dst_x: i32, dst_y: i32) {
    let img_w = rgba.width() as i32;
    let img_h = rgba.height() as i32;
    let buf = rgba.as_mut();
    let stride = (img_w * 4) as usize;
    for y in 0..sprite.height {
        let py = dst_y + y;
        if py < 0 || py >= img_h {
            continue;
        }
        let row_start = (py as usize) * stride;
        for x in 0..sprite.width {
            let px = dst_x + x;
            if px < 0 || px >= img_w {
                continue;
            }
            let src_idx = ((y * sprite.width + x) * 4) as usize;
            let s = &sprite.pixels[src_idx..src_idx + 4];
            let alpha = s[3] as u32;
            if alpha == 0 {
                continue;
            }
            let dst_idx = row_start + (px as usize) * 4;
            if alpha == 255 {
                buf[dst_idx] = s[0];
                buf[dst_idx + 1] = s[1];
                buf[dst_idx + 2] = s[2];
            } else {
                let inv = 255 - alpha;
                buf[dst_idx] =
                    ((s[0] as u32 * alpha + buf[dst_idx] as u32 * inv) / 255) as u8;
                buf[dst_idx + 1] =
                    ((s[1] as u32 * alpha + buf[dst_idx + 1] as u32 * inv) / 255) as u8;
                buf[dst_idx + 2] =
                    ((s[2] as u32 * alpha + buf[dst_idx + 2] as u32 * inv) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn blit_clips_out_of_bounds_left() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]));
        let sprite = Sprite {
            pixels: vec![0xFF, 0, 0, 255, 0xFF, 0, 0, 255],
            width: 2,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
        };
        blit(&mut img, &sprite, -1, 0);
        assert_eq!(img.get_pixel(0, 0).0, [0xFF, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn blit_skips_transparent_pixels() {
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 255]));
        let sprite = Sprite {
            pixels: vec![0, 0, 0, 0, 0xFF, 0, 0, 255],
            width: 2,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
        };
        blit(&mut img, &sprite, 0, 0);
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0xFF, 0, 0, 255]);
    }

    #[test]
    fn blit_blends_half_alpha() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
        let sprite = Sprite {
            pixels: vec![0xFF, 0xFF, 0xFF, 128],
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
        };
        blit(&mut img, &sprite, 0, 0);
        let p = img.get_pixel(0, 0).0;
        assert!((p[0] as i32 - 128).abs() <= 1);
        assert!((p[1] as i32 - 128).abs() <= 1);
        assert!((p[2] as i32 - 128).abs() <= 1);
    }
}
