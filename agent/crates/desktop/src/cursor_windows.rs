//! Windows OS cursor polling + sprite decoding.
//!
//! Replaces the previous in-frame compositor with a sideband design: a
//! 60 Hz polling task reads `GetCursorInfo` and forwards pose + shape over
//! a WebRTC DataChannel, decoupling cursor latency from the video frame
//! rate. The client renders the sprite at the reported pose at network
//! RTT instead of waiting for the next captured frame.
//!
//! Sprite decoding handles three Windows cursor formats:
//! - 32-bpp colour with premultiplied alpha
//! - 32-bpp colour with no alpha + 1-bpp AND mask
//! - 1-bpp monochrome (AND + XOR mask stacked vertically)
//!
//! Sprites are cached by HCURSOR pointer-as-u64 (the OS reuses the same
//! handle per shape). Cache cap 32; clears fully on overflow.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, CopyIcon, DestroyCursor, GetCursorInfo, GetIconInfo, HCURSOR,
    ICONINFO,
};

/// Snapshot of the OS cursor: where it is and which shape it's wearing.
/// Coordinates are in the calling process's DPI awareness — the agent is
/// DPI-unaware (see `permissions.rs::normalize_to_logical`), so callers
/// can treat them as LOGICAL screen coords and normalize against
/// `MonitorInfo::{width, height}` to produce `[0, 1]` for the wire.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub x: i32,
    pub y: i32,
    /// HCURSOR pointer reinterpreted as `u64` — stable per cursor shape on
    /// a given session. Sender uses this to detect shape changes; receiver
    /// uses it as the sprite cache key.
    pub id: u64,
    pub hidden: bool,
}

/// Decoded cursor sprite, ready for wire transmission.
#[derive(Debug)]
pub struct CursorSprite {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    /// Top-down RGBA, tightly packed. `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

struct CachedSprite {
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    rgba: Vec<u8>,
}

static SPRITE_CACHE: OnceLock<Mutex<HashMap<u64, CachedSprite>>> = OnceLock::new();

fn sprite_cache() -> &'static Mutex<HashMap<u64, CachedSprite>> {
    SPRITE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read the live cursor state. Returns `None` only if `GetCursorInfo`
/// itself fails — a hidden / suppressed cursor still returns `Some` with
/// `hidden = true`.
pub fn poll_state() -> Option<CursorState> {
    let ci = unsafe {
        let mut ci: CURSORINFO = std::mem::zeroed();
        ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        if GetCursorInfo(&mut ci) == 0 {
            return None;
        }
        ci
    };
    let hidden = (ci.flags & CURSOR_SHOWING) == 0 || ci.hCursor.is_null();
    Some(CursorState {
        x: ci.ptScreenPos.x,
        y: ci.ptScreenPos.y,
        id: ci.hCursor as u64,
        hidden,
    })
}

/// Decode (or fetch from cache) the sprite for a given HCURSOR id.
/// Returns `None` if the cursor handle is null or any GDI call fails.
pub fn fetch_sprite(id: u64) -> Option<CursorSprite> {
    if id == 0 {
        return None;
    }
    let mut guard = sprite_cache().lock().unwrap_or_else(|e| e.into_inner());
    if !guard.contains_key(&id) {
        let sprite = unsafe { decode_cursor(id as HCURSOR) }?;
        if guard.len() >= 32 {
            guard.clear();
        }
        guard.insert(id, sprite);
    }
    let s = guard.get(&id).expect("just inserted");
    Some(CursorSprite {
        width: s.width,
        height: s.height,
        hotspot_x: s.hotspot_x,
        hotspot_y: s.hotspot_y,
        rgba: s.rgba.clone(),
    })
}

unsafe fn decode_cursor(hcursor: HCURSOR) -> Option<CachedSprite> {
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

    if !info.hbmColor.is_null() {
        unsafe { DeleteObject(info.hbmColor) };
    }
    if !info.hbmMask.is_null() {
        unsafe { DeleteObject(info.hbmMask) };
    }
    unsafe { DestroyCursor(copy) };
    result
}

unsafe fn decode_icon_info(info: &ICONINFO) -> Option<CachedSprite> {
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

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let ok = if !info.hbmColor.is_null() {
        unsafe { fill_color_sprite(dc, info, width, height, &mut pixels) }
    } else {
        unsafe { fill_mono_sprite(dc, info.hbmMask, width, height, &mut pixels) }
    };
    unsafe { DeleteDC(dc) };
    if !ok {
        return None;
    }

    Some(CachedSprite {
        width: width as u32,
        height: height as u32,
        hotspot_x: info.xHotspot,
        hotspot_y: info.yHotspot,
        rgba: pixels,
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_state_returns_some_or_handles_failure_gracefully() {
        // Smoke: never panic, never deadlock. Result varies by host state.
        let _ = poll_state();
    }

    #[test]
    fn fetch_sprite_zero_id_is_none() {
        assert!(fetch_sprite(0).is_none());
    }
}
