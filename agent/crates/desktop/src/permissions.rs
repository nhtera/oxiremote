/// Platform permission probing for desktop capture and accessibility.
use tracing::warn;

/// One-time install of a panic-hook filter that suppresses screen-capture
/// panics (xcap / libwayshot UnsupportedVersion etc.) and routes them to
/// tracing instead of stderr. Default behaviour is preserved for every
/// other panic.
///
/// Why this exists: libwayshot 0.3.2 panics on Wayland compositors that
/// don't expose wlr-screencopy (notably WSLg's RDP backend). Without this
/// filter the default panic handler writes to stderr while the TUI owns
/// the alternate screen, corrupting the render.
///
/// Installed at most once per process via `std::sync::Once` — avoids the
/// global-hook race that a per-call `take_hook`/`set_hook` swap would
/// introduce when concurrent probes run (host dashboard polls
/// `/api/agent/permissions` every 5 s).
///
/// NOTE: if any other code installs a panic hook AFTER this filter, it must
/// chain through `prev_hook` from its own `take_hook()` to keep this filter
/// alive. Today nothing else installs a hook in the agent.
#[cfg(target_os = "linux")]
fn install_capture_panic_filter() {
    use std::panic;
    use std::sync::Once;
    static FILTER: Once = Once::new();
    FILTER.call_once(|| {
        let prev = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let from_capture = info
                .location()
                .map(|l| {
                    let f = l.file();
                    f.contains("libwayshot") || f.contains("xcap")
                })
                .unwrap_or(false);
            if from_capture {
                tracing::debug!("screen capture probe panic suppressed");
                return;
            }
            prev(info);
        }));
    });
}

/// Detects WSL environments where the Wayland compositor (WSLg) doesn't
/// expose wlr-screencopy. Memoized — /proc/version doesn't change at runtime.
/// "microsoft" is the canonical marker stamped by Microsoft on every WSL
/// kernel build; we don't match "wsl" alone since that substring can appear
/// in unrelated build hosts or user names.
#[cfg(target_os = "linux")]
fn is_wsl() -> bool {
    use std::sync::OnceLock;
    static WSL: OnceLock<bool> = OnceLock::new();
    *WSL.get_or_init(|| {
        std::fs::read_to_string("/proc/version")
            .map(|v| v.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
    })
}

/// Basic monitor descriptor — plain data, no xcap types exposed to callers.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub id: usize,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

/// List all available monitors as plain `MonitorInfo` structs.
/// Returns an empty vec if xcap cannot enumerate monitors.
///
/// `width` / `height` are LOGICAL pixels on every platform. xcap's
/// `Monitor::width()` returns logical points on macOS but physical pixels
/// on Windows at fractional DPI (e.g. 1920×1080 at 125%) — we normalize
/// here so the rest of the pipeline (`encode::quality_resize` output,
/// enigo's DPI-unaware `SendInput`, the SPA's `Capabilities` canvas
/// sizing) all speak the same coordinate space.
pub fn list_monitors() -> Vec<MonitorInfo> {
    xcap::Monitor::all()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let raw_w = m.width().unwrap_or(0);
            let raw_h = m.height().unwrap_or(0);
            let scale = m.scale_factor().unwrap_or(1.0).max(1.0);
            let (width, height) = normalize_to_logical(raw_w, raw_h, scale);
            MonitorInfo {
                id: i,
                label: if i == 0 { "Primary".into() } else { "Monitor".into() },
                width,
                height,
            }
        })
        .collect()
}

/// Convert xcap's `Monitor::width()/height()` return to LOGICAL pixels.
///
/// macOS / Linux: xcap already reports logical → pass through (no-op).
/// Windows: xcap reports physical at fractional DPI → divide by scale_factor.
///
/// The result is what enigo's DPI-unaware `GetSystemMetrics`-based mapping
/// expects, what `encode::quality_resize` produces, and what the SPA needs
/// to size its `<canvas>` to match the actual encoded frame. Without this
/// normalization, `Capabilities` advertises physical (1920×1080) while the
/// encoded frame is logical (1536×864) — JPEG renders into 80% of the
/// canvas (small-screen symptom) and H.264 mouse coords overshoot host by
/// `scale_factor` (cursor lands in the wrong place).
#[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
fn normalize_to_logical(raw_w: u32, raw_h: u32, scale: f32) -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        if scale > 1.0 + f32::EPSILON {
            let lw = ((raw_w as f32) / scale).round() as u32;
            let lh = ((raw_h as f32) / scale).round() as u32;
            return (lw.max(1), lh.max(1));
        }
    }
    (raw_w, raw_h)
}

/// Availability status of desktop features on the current system.
#[derive(Debug, Clone)]
pub struct PermissionStatus {
    /// Whether screen recording / capture is permitted.
    pub screen_recording: bool,
    /// Whether accessibility (input injection) is permitted.
    pub accessibility: bool,
}

impl PermissionStatus {
    /// Probe current permission state. Non-blocking: no UI prompts triggered.
    pub fn check() -> PermissionStatus {
        let screen_recording = probe_screen_recording();
        let accessibility = probe_accessibility();
        PermissionStatus {
            screen_recording,
            accessibility,
        }
    }
}

/// Returns `true` if screen capture is available and permitted on this machine.
///
/// Linux headless (no DISPLAY / WAYLAND_DISPLAY): always false.
/// macOS / Windows: attempts Monitor::all() + one capture; on error returns false.
pub fn desktop_available() -> bool {
    // Linux headless guard — no display server means no capture possible.
    #[cfg(target_os = "linux")]
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        return false;
    }

    probe_screen_recording()
}

fn probe_screen_recording() -> bool {
    use xcap::Monitor;

    let monitors = match Monitor::all() {
        Ok(ms) if !ms.is_empty() => ms,
        Ok(_) => {
            warn!("no monitors found");
            return false;
        }
        Err(err) => {
            warn!(error = %err, "Monitor::all() failed");
            return false;
        }
    };
    let monitor = monitors.into_iter().next().unwrap();

    // Linux: capture can block on the Wayland PipeWire portal handshake, so
    // probe in a worker thread with a hard deadline. macOS / Windows are
    // synchronous and fast — and on Windows `xcap::Monitor` isn't `Send`
    // (HMONITOR is `*mut c_void`), so the thread-spawn version won't compile.
    #[cfg(target_os = "linux")]
    {
        // Install the panic-hook filter unconditionally on Linux — covers both
        // the WSL early-exit path (defensive: if a future xcap version makes
        // Monitor::all() panic on WSLg, list_monitors() would otherwise be
        // unprotected) and the non-WSL probe below. Idempotent via Once.
        install_capture_panic_filter();

        // WSLg's RDP-backed Wayland doesn't speak wlr-screencopy; libwayshot
        // panics on first capture. Skip the probe — capture is unavailable
        // and we know it without spawning a thread. Log once per process to
        // avoid filling the headless-mode stderr / agent.log every 5 s when
        // the host dashboard is open.
        if is_wsl() {
            use std::sync::Once;
            static LOGGED: Once = Once::new();
            LOGGED.call_once(|| {
                warn!("WSL detected: screen capture not supported on WSLg compositor");
            });
            return false;
        }

        // `move` on the inner closure keeps `monitor`'s Drop inside the
        // catch_unwind window in case Drop itself panics on a torn-state
        // monitor (only helps when capture_image returns normally; a
        // panic-in-Drop during capture_image's unwind is `abort` per Rust).

        use std::panic::{self, AssertUnwindSafe};
        use std::sync::mpsc;
        use std::time::Duration;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(move || {
                monitor.capture_image().is_ok()
            }))
            .unwrap_or(false);
            let _ = tx.send(result);
        });
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => true,
            Ok(false) => {
                warn!("screen capture probe failed (display unavailable, denied, or unsupported compositor)");
                false
            }
            Err(_) => {
                warn!("screen capture probe timed out (3s)");
                false
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        match monitor.capture_image() {
            Ok(_) => true,
            Err(err) => {
                warn!(error = %err, "screen capture probe failed");
                false
            }
        }
    }
}

fn probe_accessibility() -> bool {
    // On macOS, AXIsProcessTrusted() is the authoritative check.
    // On Linux/Windows, assume available when a display is present.
    #[cfg(target_os = "macos")]
    {
        // Safety: AXIsProcessTrusted is a simple boolean query with no side
        // effects and no memory allocation — safe to call at any time.
        unsafe extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows: 1920x1080 physical at 125% DPI must normalize to 1536x864
    // logical. The downstream contract is:
    //   • SPA Capabilities width/height match the encoded-frame dims
    //     produced by encode::quality_resize (which divides by scale_factor).
    //   • enigo's DPI-unaware SendInput interprets coords against
    //     GetSystemMetrics, which returns logical for non-DPI-aware procs.
    // Mismatch causes JPEG "small screen" (canvas > frame) and H.264 cursor
    // overshoot (input_px * scale = host_px) — both addressed here.
    #[test]
    #[cfg(target_os = "windows")]
    fn normalize_windows_125_dpi() {
        assert_eq!(normalize_to_logical(1920, 1080, 1.25), (1536, 864));
        assert_eq!(normalize_to_logical(2560, 1440, 1.5), (1707, 960));
        // 100% scale is a no-op on the conversion branch.
        assert_eq!(normalize_to_logical(1920, 1080, 1.0), (1920, 1080));
    }

    // Non-Windows: pass-through regardless of scale (xcap already returns
    // logical on macOS / Linux). Guards against accidental cascade if the
    // cfg gate is ever removed.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn normalize_passthrough_off_windows() {
        assert_eq!(normalize_to_logical(1512, 982, 2.0), (1512, 982));
        assert_eq!(normalize_to_logical(1920, 1080, 1.25), (1920, 1080));
    }
}
