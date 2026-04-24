/// Platform permission probing for desktop capture and accessibility.
use tracing::warn;

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
    use std::sync::mpsc;
    use std::time::Duration;
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

    // Run the capture on a worker thread with a hard deadline. Guards against
    // Linux PipeWire portal handshakes that can block arbitrarily long.
    let monitor = monitors.into_iter().next().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(monitor.capture_image().is_ok());
    });

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(true) => true,
        Ok(false) => {
            warn!("screen capture probe failed (TCC denied or display unavailable)");
            false
        }
        Err(_) => {
            warn!("screen capture probe timed out (3s)");
            false
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
