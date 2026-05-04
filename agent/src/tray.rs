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

/// Programmatically paint a 32×32 icon. Two tones so it matches the
/// idle/active visual without shipping binary PNG assets in the repo.
fn make_icon(active: bool) -> Result<Icon> {
    const SIZE: u32 = 32;
    let (r, g, b) = if active {
        (0xFF, 0x8C, 0x00) // idle → orange accent when a device needs attention
    } else {
        (0xAA, 0xAA, 0xAA) // neutral grey when healthy/idle
    };
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Filled disc with 2px padding so the icon reads at menu-bar scale.
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let d2 = dx * dx + dy * dy;
            if d2 <= 14 * 14 {
                rgba.extend_from_slice(&[r, g, b, 0xFF]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| anyhow::anyhow!("tray icon: {e}"))
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
        .with_icon(make_icon(false)?)
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

