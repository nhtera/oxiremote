// Interactive terminal UI for bare `oxiremote` invocations. Runs on the main
// thread; the HTTP server lives on a sibling tokio runtime. Cleanup uses
// `scopeguard` semantics (Drop on `TerminalGuard`) so a panic never leaves the
// host terminal in raw+alt-screen state.

use std::io;
use std::sync::Arc;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::events::EventBus;

pub mod approval;
pub mod dashboard;
pub mod logs;
pub mod menu;
pub mod step_progress;

use menu::MenuChoice;

/// Restores the terminal on drop — even if a panic unwinds through the TUI
/// loop. Without this, a ratatui crash leaves the shell in raw mode.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
        let _ = disable_raw_mode();
    }
}

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn new_terminal() -> Result<Term> {
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Main entry. Owns the terminal lifecycle; returns on "Exit" menu selection
/// or Ctrl+C.
pub fn run_tui(event_bus: Arc<EventBus>) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut term = new_terminal()?;

    loop {
        match menu::run_menu(&mut term)? {
            MenuChoice::OpenWebUi => {
                // Best-effort browser launch; fall through to dashboard so the
                // user keeps event visibility even if open(1) is missing.
                let _ = open::that("http://localhost:8787/agent");
                dashboard::run_dashboard(&mut term, event_bus.clone())?;
            }
            MenuChoice::TerminalUi => {
                dashboard::run_dashboard(&mut term, event_bus.clone())?;
            }
            MenuChoice::Exit => break,
        }
    }

    Ok(())
}
