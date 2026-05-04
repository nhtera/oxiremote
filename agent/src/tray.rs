// System tray integration. Built on `tray-icon` 0.19. The tray lives on the
// main thread on macOS/Windows (platform constraint); the tokio runtime must
// already be running on a sibling thread before `run_event_loop` is called.
//
// Menu mirrors 9remote's pattern: a non-clickable status header that flips
// between "Local only" and "Tunnel: <host>", plus "Open Web UI" and
// "Shutdown". Updates are driven by `AgentEvent::TunnelUrlChanged` /
// `TunnelDown` so the operator sees the current reachability without
// opening the dashboard.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::events::{AgentEvent, EventBus};
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
    // Held so we can rewrite the status text from bus events.
    status_item: MenuItem,
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
        status_item: status,
    })
}

/// Blocks the caller (typically the main thread on macOS/Windows) and
/// dispatches menu clicks until the "Shutdown" item is chosen. The status
/// header re-renders on every TunnelUrlChanged / TunnelDown so the menu
/// reflects current reachability without polling.
pub fn run_event_loop(handle: &TrayHandle, event_bus: Arc<EventBus>) {
    let menu_rx = MenuEvent::receiver();
    let mut bus_rx = event_bus.subscribe();

    loop {
        // Drain bus events (non-blocking) — only the tunnel transitions
        // affect the menu; everything else is dropped to keep the broadcast
        // channel from lagging other subscribers (notifier, dashboard SSE).
        while let Ok(event) = bus_rx.try_recv() {
            match event {
                AgentEvent::TunnelUrlChanged { url } => {
                    let host = host_only(&url);
                    handle
                        .status_item
                        .set_text(format!("OxiRemote · {host}"));
                }
                AgentEvent::TunnelDown { .. } => {
                    handle.status_item.set_text(initial_status_text());
                }
                _ => {}
            }
        }

        if let Ok(evt) = menu_rx.try_recv() {
            let id = evt.id.0;
            if id == handle.ids.open_web {
                let _ = open::that(format!("http://localhost:{AGENT_PORT}/agent"));
            } else if id == handle.ids.shutdown {
                crate::tui::restore_terminal_if_active();
                std::process::exit(0);
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Strip scheme + path so the menu shows just `oxiremote.example.com` in
/// the limited width of a menu-bar item.
fn host_only(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}
