// System tray integration. Built on `tray-icon` 0.19. The tray lives on the
// main thread on macOS/Windows (platform constraint); the tokio runtime must
// already be running on a sibling thread before `run_event_loop` is called.
//
// Menu layout: a non-clickable status header showing the agent port, plus
// "Open Web UI" and "Shutdown" — minimal so the operator never has to dig.

#[cfg(not(target_os = "macos"))]
use std::time::Duration;

use anyhow::Result;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::AGENT_PORT;

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
}

struct MenuIds {
    open_web: String,
    shutdown: String,
}

fn initial_status_text() -> String {
    format!("OxiRemote (Port {AGENT_PORT}) · Local only")
}

pub fn build_tray() -> Result<TrayHandle> {
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
    })
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
        macos_run();
    }

    #[cfg(not(target_os = "macos"))]
    {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}

/// Bootstrap an NSApplication, set Accessory activation policy (menu-bar
/// icon only, no Dock entry), and hand the main thread to `[NSApp run]`.
/// The run loop only returns when the process is asked to terminate.
#[cfg(target_os = "macos")]
fn macos_run() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let mtm = objc2::MainThreadMarker::new()
        .expect("tray::run_event_loop must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Block forever — `[NSApp run]` returns only on terminate. Menu clicks
    // fire via the global `MenuEvent::set_event_handler` installed above.
    app.run();
}

