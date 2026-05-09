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
pub fn list_monitors() -> Vec<MonitorInfo> {
    xcap::Monitor::all()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            id: i,
            label: if i == 0 { "Primary".into() } else { "Monitor".into() },
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
        })
        .collect()
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
