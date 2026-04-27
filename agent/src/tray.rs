// System tray integration. Built on `tray-icon` 0.19. The tray lives on the
// main thread on macOS/Windows (platform constraint); the tokio runtime must
// already be running on a sibling thread before `run_tray` is called.
//
// Event loop is driven by a polling `MenuEvent::receiver()` that this module
// owns. Not wired to the default bare-invocation dispatch due to main-thread
// contention with the TUI's raw-mode stdio. Kept here to lock the API surface.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::events::EventBus;

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
    status: String,
    open_web: String,
    open_host: String,
    shutdown: String,
}

pub fn build_tray() -> Result<TrayHandle> {
    let menu = Menu::new();
    let status = MenuItem::new("OxiRemote — starting…", false, None);
    let open_web = MenuItem::new("Open Web UI", true, None);
    let open_host = MenuItem::new("Open Host Dashboard", true, None);
    let shutdown = MenuItem::new("Shutdown", true, None);

    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&open_web)?;
    menu.append(&open_host)?;
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
            status: status.id().0.clone(),
            open_web: open_web.id().0.clone(),
            open_host: open_host.id().0.clone(),
            shutdown: shutdown.id().0.clone(),
        },
    })
}

/// Blocks the caller (typically the main thread on macOS/Windows) and
/// dispatches menu clicks until the "Shutdown" item is chosen. Bus events
/// only drive icon state here; desktop notifications are owned by
/// `crate::notifier` and fire regardless of whether the tray is wired.
pub fn run_event_loop(handle: &TrayHandle, event_bus: Arc<EventBus>) {
    let menu_rx = MenuEvent::receiver();
    let mut bus_rx = event_bus.subscribe();

    loop {
        // Drain bus events (non-blocking) so the channel doesn't lag the
        // notifier and other subscribers. Currently no per-event behaviour;
        // future icon-tone changes (idle/active) hook in here.
        while let Ok(_event) = bus_rx.try_recv() {}

        if let Ok(evt) = menu_rx.try_recv() {
            let id = evt.id.0;
            if id == handle.ids.open_web || id == handle.ids.open_host {
                let _ = open::that("http://localhost:8787/agent");
            } else if id == handle.ids.shutdown {
                crate::tui::restore_terminal_if_active();
                std::process::exit(0);
            } else if id == handle.ids.status {
                // noop — disabled item
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}
