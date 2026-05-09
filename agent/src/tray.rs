// System tray integration. Built on `tray-icon` 0.19. The tray lives on the
// main thread on macOS/Windows (platform constraint); the tokio runtime must
// already be running on a sibling thread before `run_event_loop` is called.
//
// Menu layout: a non-clickable status header showing the agent port, plus
// "Open Web UI" and "Shutdown" — minimal so the operator never has to dig.
//
// Degraded state: when AgentEvent::Degraded fires (supervisor failed 10+
// times in 24h), a background task sends TrayCmd::Degraded to a channel
// received by the main-thread event loop, which updates the tooltip.
// Cleared on next TunnelUrlChanged.

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
use std::time::Duration;
use std::sync::Arc;
use std::sync::mpsc;

use anyhow::Result;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::AGENT_PORT;
use crate::events::{AgentEvent, EventBus};

/// Commands the background bus watcher sends to the main-thread event loop.
pub enum TrayCmd {
    /// Tunnel supervisor hit 10+ failures in 24h.
    Degraded { retry_in_secs: u64 },
    /// A new tunnel URL arrived — clear degraded state.
    TunnelUp,
}

/// 44×44 retina menu-bar monitor outline rendered from
/// `assets/menu-bar-monitor.svg` — same glyph the SPA uses in its
/// agent-header logo, kept in sync so the tray and the dashboard read
/// as the same product. Pre-rasterised via `rsvg-convert` so the binary
/// stays self-contained without pulling resvg for a single 44 px raster.
///
/// The PNG is solid black on transparent; the tray sets template-image
/// mode so AppKit auto-tints it (white in dark menu bars, black in
/// light) and respects accent / inverted modes.
const MENU_BAR_ICON_PNG: &[u8] = include_bytes!("../assets/menu-bar-monitor-44.png");

fn make_icon() -> Result<Icon> {
    let img = image::load_from_memory(MENU_BAR_ICON_PNG)
        .map_err(|e| anyhow::anyhow!("decode menu-bar icon png: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h)
        .map_err(|e| anyhow::anyhow!("tray icon: {e}"))
}

pub struct TrayHandle {
    // Held to keep the tray alive for the lifetime of the process.
    _tray: tray_icon::TrayIcon,
    ids: MenuIds,
    /// Receives TrayCmd from the background bus-watcher task.
    cmd_rx: mpsc::Receiver<TrayCmd>,
}

struct MenuIds {
    open_web: String,
    shutdown: String,
}

fn initial_status_text() -> String {
    format!("OxiRemote (Port {AGENT_PORT}) · Local only")
}

/// Allocate the channel that bridges async tunnel events to the main-thread
/// tray loop. Caller must spawn [`run_degraded_watcher`] on a tokio runtime
/// using the returned `Sender`. Splitting allocation from the spawn lets the
/// caller decide which runtime owns the watcher task — `run_with_tray` runs
/// the runtime on a sibling thread, so calling `tokio::spawn` from the main
/// thread would panic with "no reactor running".
pub fn tray_cmd_channel() -> (mpsc::Sender<TrayCmd>, mpsc::Receiver<TrayCmd>) {
    mpsc::channel::<TrayCmd>()
}

/// Subscribe to the event bus and forward Degraded / TunnelUrlChanged events
/// to the main-thread tray loop via the std mpsc channel. Must be spawned on
/// a tokio runtime — the caller is responsible (e.g. via `Runtime::spawn` or
/// `tokio::spawn` from inside an async context).
pub async fn run_degraded_watcher(bus: Arc<EventBus>, tx: mpsc::Sender<TrayCmd>) {
    let mut event_rx = bus.subscribe();
    loop {
        let ev = match event_rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        match ev {
            AgentEvent::Degraded { retry_in_secs, .. } => {
                let _ = tx.send(TrayCmd::Degraded { retry_in_secs });
            }
            AgentEvent::TunnelUrlChanged { .. } => {
                let _ = tx.send(TrayCmd::TunnelUp);
            }
            _ => {}
        }
    }
}

pub fn build_tray(cmd_rx: mpsc::Receiver<TrayCmd>) -> Result<TrayHandle> {
    let menu = Menu::new();
    let status = MenuItem::new(initial_status_text(), false, None);
    let open_web = MenuItem::new("Open Web UI", true, None);
    let shutdown = MenuItem::new("Shutdown", true, None);

    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&open_web)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&shutdown)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("OxiRemote")
        .with_icon(make_icon()?)
        // Template flag tells AppKit the icon is a monochrome silhouette so
        // it auto-tints to match the menu bar's tone — the only sane mode
        // for a CLI-style status item.
        .with_icon_as_template(true)
        .build()?;

    Ok(TrayHandle {
        _tray: tray,
        ids: MenuIds {
            open_web: open_web.id().0.clone(),
            shutdown: shutdown.id().0.clone(),
        },
        cmd_rx,
    })
}

/// Drain pending TrayCmd messages and update the tray icon tooltip.
/// Called on the main thread each iteration of the platform event loop.
fn process_tray_cmds(handle: &TrayHandle) {
    while let Ok(cmd) = handle.cmd_rx.try_recv() {
        match cmd {
            TrayCmd::Degraded { retry_in_secs } => {
                let tip = format!(
                    "OxiRemote (Port {AGENT_PORT}) · DEGRADED — tunnel unstable, retrying in {retry_in_secs}s"
                );
                let _ = handle._tray.set_tooltip(Some(tip));
            }
            TrayCmd::TunnelUp => {
                let tip = format!("OxiRemote (Port {AGENT_PORT}) · Tunnel active");
                let _ = handle._tray.set_tooltip(Some(tip));
            }
        }
    }
}

/// Blocks the caller (typically the main thread on macOS/Windows) and
/// dispatches menu clicks until the "Shutdown" item is chosen.
///
/// On macOS the host process must own a running NSApplication for the
/// status item to render in the menu bar — we spin one up here and route
/// menu clicks via `MenuEvent::set_event_handler` so the NSApp run loop
/// can stay blocking. On other platforms we fall back to parking the
/// thread; the click callback fires on its own dispatcher.
pub fn run_event_loop(handle: &TrayHandle) {
    // Wire menu clicks via the global handler — fires from whichever thread
    // the platform delivers menu events on (main on macOS).
    let open_web_id = handle.ids.open_web.clone();
    let shutdown_id = handle.ids.shutdown.clone();
    MenuEvent::set_event_handler(Some(move |evt: MenuEvent| {
        let id = evt.id.0;
        if id == open_web_id {
            let _ = open::that(format!("http://localhost:{AGENT_PORT}/agent"));
        } else if id == shutdown_id {
            crate::tui::restore_terminal_if_active();
            std::process::exit(0);
        }
    }));

    #[cfg(target_os = "macos")]
    {
        macos_run(handle);
    }

    #[cfg(target_os = "windows")]
    {
        windows_run(handle);
    }

    // Linux fallback. tray-icon dispatches click events via its own thread
    // on Linux (gtk/libappindicator), so a periodic sleep keeps the process
    // alive while we drain TrayCmd messages each iteration.
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        loop {
            process_tray_cmds(handle);
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

/// Bootstrap an NSApplication, set Accessory activation policy (menu-bar
/// icon only, no Dock entry), and pump NSRunLoop in 100ms slices so we can
/// drain TrayCmd messages between platform event dispatches.
///
/// `NSRunLoop::runUntilDate:` runs the loop until the given date expires OR
/// an input source fires — whichever comes first. This gives tooltip updates
/// a maximum latency of ~100ms, which is imperceptible to the operator.
#[cfg(target_os = "macos")]
fn macos_run(handle: &TrayHandle) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{NSDate, NSRunLoop};

    let mtm = objc2::MainThreadMarker::new()
        .expect("tray::run_event_loop must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    loop {
        process_tray_cmds(handle);
        // Pump the run loop for up to 100ms — processes menu events, then
        // returns so we can drain any pending TrayCmd messages.
        let rl = NSRunLoop::mainRunLoop();
        let deadline = NSDate::dateWithTimeIntervalSinceNow(0.1);
        rl.runUntilDate(&deadline);
    }
}

/// Standard Win32 message pump with a periodic PeekMessage drain for TrayCmd.
/// `PeekMessage` with PM_REMOVE is non-blocking so we can dispatch platform
/// messages AND service TrayCmd in the same tight loop.
#[cfg(target_os = "windows")]
fn windows_run(handle: &TrayHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
    };

    // Safety: MSG is a plain C struct; zeroed init is the documented pattern.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        loop {
            // Drain all pending Windows messages (non-blocking).
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // Process any pending TrayCmd from the supervisor.
            process_tray_cmds(handle);
            // Yield briefly so we don't pin a CPU core at 100%.
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
