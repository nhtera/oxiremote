// Interactive terminal UI for bare `oxiremote` invocations. Runs on the main
// thread; the HTTP server lives on a sibling tokio runtime. Cleanup uses
// `scopeguard` semantics (Drop on `TerminalGuard`) so a panic never leaves the
// host terminal in raw+alt-screen state.

use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::Thread;

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
pub mod menu;
pub mod step_progress;

use menu::MenuChoice;

/// True while the TUI owns the terminal (raw mode + alternate screen + mouse
/// capture enabled). Read by [`restore_terminal_if_active`] so external exit
/// paths (`process::exit`, panic hook, libc atexit) can cleanly tear the
/// terminal back down — `TerminalGuard::drop` only runs on normal unwinding,
/// not on `process::exit`, so without this the shell ends up still in
/// mouse-tracking mode and prints SGR motion reports as plain text.
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static ATEXIT_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Main-thread handle stashed when the TUI menu picks "Open Web UI
/// (background)" and parks. The server thread's SIGINT handler unparks via
/// [`wake_background_main`] so Ctrl+C exits the process — without this, tokio
/// consumes SIGINT for axum's graceful shutdown and the main thread parks
/// forever even after the server thread returns.
static BACKGROUND_MAIN: OnceLock<Thread> = OnceLock::new();
static BACKGROUND_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Called from the server thread's shutdown handler to release the parked
/// main thread. Safe to call repeatedly and from any thread.
pub fn wake_background_main() {
    BACKGROUND_SHUTDOWN.store(true, Ordering::SeqCst);
    if let Some(t) = BACKGROUND_MAIN.get() {
        t.unpark();
    }
}

/// Spawn `oxiremote --tray` as a detached child (setsid + closed stdio) so
/// the launching shell returns to its prompt while the agent keeps running
/// in the background with a menu-bar icon. Prints the child's PID for the
/// `pkill`-from-another-shell escape hatch.
///
/// Errors are reported to stderr but never abort — the parent has already
/// dropped its TerminalGuard, so we can't gracefully fall back to the TUI.
fn spawn_detached_tray() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("(could not locate oxiremote binary: {err})");
            return;
        }
    };
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--tray")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    match cmd.spawn() {
        Ok(child) => {
            println!(
                "OxiRemote running. Right-click the menu-bar icon to quit. (PID: {})",
                child.id()
            );
            println!("  Dashboard: http://localhost:8787/agent");
        }
        Err(err) => {
            eprintln!("(failed to spawn detached tray: {err})");
        }
    }
}

/// Best-effort terminal restore. Idempotent; safe to call multiple times and
/// from any thread. No-op when the TUI was never entered.
pub fn restore_terminal_if_active() {
    if !TUI_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    // Disable raw mode BEFORE leaving the alternate screen — otherwise OPOST
    // stays off and any subsequent println!() lines stair-step (LF moves the
    // cursor down without returning to column 0). Order matches ratatui's
    // canonical restore path.
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
}

#[cfg(unix)]
extern "C" fn atexit_restore() {
    restore_terminal_if_active();
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        TUI_ACTIVE.store(true, Ordering::SeqCst);
        // libc is a unix-only dep (see agent/Cargo.toml). Windows still gets
        // restore via the Drop impl on TerminalGuard + the panic hook in
        // run_with_panic_hook — atexit was a belt-and-braces fallback for the
        // POSIX abort()/exit() paths that bypass Drop on unix.
        #[cfg(unix)]
        if !ATEXIT_REGISTERED.swap(true, Ordering::SeqCst) {
            unsafe {
                libc::atexit(atexit_restore);
            }
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal_if_active();
    }
}

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn new_terminal() -> Result<Term> {
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Main entry. Owns the terminal lifecycle; returns on "Exit" menu selection
/// or Ctrl+C. `db_path` is threaded through so the dashboard can read/refresh
/// the active OTK directly (server thread shares the same SQLite file).
/// `discovery_url` + `discovery_temp_key` are populated when
/// `OXI_DISCOVERY_URL` is set; the dashboard uses them to switch the QR
/// payload to the cross-origin discovery form.
pub fn run_tui(
    event_bus: Arc<EventBus>,
    db_path: PathBuf,
    discovery_url: Option<String>,
    discovery_temp_key: Arc<std::sync::RwLock<Option<String>>>,
) -> Result<()> {
    let mut _guard = TerminalGuard::enter()?;
    let mut term = new_terminal()?;

    // Check for a newer release once at startup. 5s network timeout in
    // `check_latest` keeps it bounded; cached here so re-entering the menu
    // from the dashboard doesn't re-probe on every loop iteration.
    let update_version = crate::update::check_latest();

    loop {
        match menu::run_menu(&mut term, update_version.as_deref())? {
            MenuChoice::OpenWebUi => {
                // Restore the host terminal first so the post-exit text lands
                // on the normal screen, then daemonize: spawn a detached
                // `--tray` child and let this process exit. The child owns the
                // tunnel + tray going forward; the launching shell returns
                // to its prompt.
                drop(term);
                drop(_guard);
                spawn_detached_tray();
                let _ = open::that("http://localhost:8787/agent");
                std::process::exit(0);
            }
            MenuChoice::TerminalUi => {
                dashboard::run_dashboard(
                    &mut term,
                    event_bus.clone(),
                    db_path.clone(),
                    discovery_url.clone(),
                    discovery_temp_key.clone(),
                )?;
            }
            MenuChoice::UpdateAvailable(_ver) => {
                // Temporarily leave alternate screen so the update output is
                // visible on the normal scrollback buffer, then restore TUI.
                drop(_guard);
                let _ = crate::update::run();
                // Re-enter TUI after the update completes (or errors out).
                _guard = TerminalGuard::enter()?;
                term = new_terminal()?;
            }
            MenuChoice::Exit => break,
        }
    }

    Ok(())
}
