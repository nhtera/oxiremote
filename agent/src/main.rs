mod agent_api;
mod agent_detector;
mod env_defaults;
mod approval;
mod auth;
mod autostart;
mod db;
mod discovery;
mod edge_health_monitor;
mod events;
mod files;
mod files_activity;
mod files_search;
mod files_upload;
mod git;
mod host;
mod health_check;
mod host_api;
mod http_pages;
mod instance_lock;
mod local_sites;
mod notifier;
mod notify_cli;
mod one_time_keys;
mod preview;
mod preview_token;
mod proxy;
mod push;
mod push_api;
mod security;
mod settings;
mod telemetry;
mod tracing_setup;
mod update;
mod static_files;
#[cfg(feature = "desktop")]
mod desktop_service;
#[cfg(feature = "desktop")]
mod desktop_ws;
#[cfg(feature = "desktop")]
mod desktop_ws_capture;
// Pipeline selection — operator env flag + client capability AND.
#[cfg(feature = "desktop")]
mod pipeline_selection;
// H.264 pipeline — gated so the JPEG default build doesn't depend on the
// VT/OpenH264 toolchain.
#[cfg(feature = "h264")]
mod video_pipeline;
// Audio pipeline — gated on the agent's `audio` feature (forwards to
// desktop/audio). Off by default until phase-02a + 02b both ship.
#[cfg(feature = "audio")]
mod desktop_audio_pipeline;
mod terminal_api;
mod terminal_buffer;
mod terminal_pty;
mod terminal_ws;
mod tray;
mod tui;
mod tunnel;
mod tunnel_named;
mod heartbeat;
mod tunnel_supervisor;
#[cfg(target_os = "windows")]
mod win_jobs;
mod workspaces;

use std::{
    collections::{HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock},
};

use anyhow::Context;
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use crate::events::{AgentEvent, EventBus};
use crate::files_activity::FilesActivityMap;
use crate::host::HostInfo;
use crate::local_sites::LocalSitesCache;
use crate::preview::{PreviewHealth, PreviewTarget};
use crate::push::VapidKeys;
use crate::security::rate_limit::RateLimiter;
use crate::telemetry::TelemetryState;
use crate::terminal_pty::TerminalSession;

#[cfg(feature = "desktop")]
use crate::desktop_service::DesktopService;

pub const AGENT_PORT: u16 = 8787;

/// Ring-buffer cap for `AppState::recent_logs`. Lets the `/agent/logs` page
/// hydrate with backfill instead of starting empty when the operator opens it
/// after the agent has already been running for a while.
pub const LOG_RING_CAP: usize = 200;

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    pub signing_key: Vec<u8>,
    pub secure_cookies: bool,
    pub terminal_sessions: Arc<DashMap<String, Arc<TerminalSession>>>,
    pub preview_targets: DashMap<String, PreviewTarget>,
    pub preview_health: DashMap<String, PreviewHealth>,
    pub local_sites: LocalSitesCache,
    /// Set of localhost ports operators have opted into for the
    /// `/proxy/<port>/*` reverse proxy. Mirrored to the `proxy_allowed_ports`
    /// settings row so toggles survive restarts.
    pub proxy_allowed_ports: Arc<StdRwLock<HashSet<u16>>>,
    pub pairing_attempts: DashMap<String, i64>,
    pub workspace_root: PathBuf,
    pub host_info: HostInfo,
    pub vapid_keys: Arc<VapidKeys>,
    pub notify_token: String,
    pub http_client: reqwest::Client,
    pub preview_client: reqwest::Client,
    pub rate_limiter: Arc<RateLimiter>,
    pub event_bus: Arc<EventBus>,
    pub tunnel_url: Arc<std::sync::RwLock<Option<String>>>,
    /// Latest `TunnelStepChanged` event, mirrored for SSE late-joiners. The
    /// broadcast bus has no replay; without this, a page reload mid-startup
    /// shows a stale "Preparing" card forever.
    pub latest_tunnel_step: Arc<StdRwLock<Option<AgentEvent>>>,
    /// Bounded ring buffer of `LogEntry` events. Same rationale — `/agent/logs`
    /// hydrates from this on mount, then streams new entries via SSE.
    pub recent_logs: Arc<StdMutex<VecDeque<AgentEvent>>>,
    /// Whether desktop capture is available and permitted on this machine.
    /// Probed once at boot via `desktop::desktop_available()`.
    pub desktop_available: bool,
    /// Active desktop session registry. `None` when `desktop_available` is false
    /// or the `desktop` feature is disabled.
    #[cfg(feature = "desktop")]
    pub desktop_service: Option<Arc<DesktopService>>,
    /// Stub field so non-desktop builds compile without the feature flag.
    #[cfg(not(feature = "desktop"))]
    pub desktop_service: Option<()>,
    /// Cloudflare discovery worker base URL (`OXI_DISCOVERY_URL`). Resolved via
    /// `env_defaults::discovery_url()`: defaults to the bundled worker URL when
    /// the env var is unset; `None` only when the user explicitly clears it.
    pub discovery_url: Option<String>,
    /// Whether the agent is running in Quick Tunnel mode (vs Named Tunnel).
    /// Set once at boot; used by `/api/host` so the SPA can show a banner when
    /// discovery is disabled AND the tunnel URL rotates per restart.
    pub is_quick_tunnel: bool,
    /// Named-tunnel hostname from `tunnel.toml` (the `hostname` field).
    /// Populated only when a named-tunnel config exists AND the hostname field
    /// is set. `None` for Quick Tunnel users. Exposed via `/api/host` so the
    /// SPA's URL allowlist can accept this domain without operator config.
    pub named_tunnel_hostname: Option<String>,
    /// Public web app base URL (`OXI_WEB_URL`). When set, QR payloads and the
    /// dashboard share-link point here (e.g.
    /// `https://remote.example.com/login?k=<otk>`) instead of the per-host
    /// tunnel — phones scan into the user-facing SPA which then resolves the
    /// tunnel URL via the discovery worker. Independent of `discovery_url`:
    /// the worker handles lookups, the SPA URL is where humans land.
    pub web_url: Option<String>,
    /// Latest temp key issued by the discovery worker — populated after every
    /// `TunnelUrlChanged` when discovery is enabled. Read by the TUI QR pane
    /// (and Phase 3 will surface it to the SPA bootstrap).
    pub discovery_temp_key: Arc<StdRwLock<Option<String>>>,
    /// Notified by `POST /api/agent/tunnel/disconnect` to gracefully tear
    /// down the cloudflared child without exiting the agent process. The
    /// tunnel-spawn task selects on this; reaching it kills the child,
    /// emits `TunnelDisconnected`, and returns.
    pub tunnel_shutdown: Arc<tokio::sync::Notify>,

    /// Operator telemetry counters: bytes_proxied, reconnect_count, probe
    /// latency ring. Written by the bytes_counter middleware and tunnel event
    /// listener; read by `GET /api/agent/tunnel/telemetry`.
    pub telemetry: Arc<TelemetryState>,

    /// Per-device last-activity map for the files API. Updated on every
    /// authenticated file API call; read by `active_sessions_snapshot` to
    /// classify devices as "files_active" when touched < 30s ago.
    pub files_activity: Arc<FilesActivityMap>,

    /// Path to the cloudflared binary — used by `GET /api/agent/version` to
    /// query the cloudflared version string without re-downloading. `None`
    /// before cloudflared has been located (should not happen in practice since
    /// `server_main` ensures cloudflared before constructing `AppState`).
    pub cloudflared_path: Option<PathBuf>,

    /// Runtime-mutable CORS allowlist. Seeded at boot from `OXI_CORS_ORIGINS`
    /// and the `cors_allowed_origins` settings row; mutated by pairing
    /// handlers (auto-add the SPA's origin) and by `/api/agent/cors/origins`
    /// (manual prune). Read by the CORS middleware on every cross-origin
    /// request.
    pub cors_origins: security::cors::CorsOrigins,
}

fn default_data_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME or USERPROFILE not set")?;
    Ok(PathBuf::from(home).join(".oxiremote"))
}

fn main() -> anyhow::Result<()> {
    // Sweep any leftover `.exe.bak` files from a previous Windows self-update
    // (the running process holds an exclusive lock on the binary until exit).
    // No-op on Unix.
    update::cleanup_stale_bak();

    // Subcommand dispatch BEFORE the tokio runtime so `notify` runs on a tiny
    // single-threaded runtime and the long-lived server uses the full one.
    let argv: Vec<String> = std::env::args().collect();
    if let Some(sub) = argv.get(1) {
        if sub == "notify" {
            let rest = argv.into_iter().skip(2).collect::<Vec<_>>();
            return notify_cli::run(rest);
        }
        if sub == "update" {
            return update::run().map(|_| ());
        }
        if sub == "--version" || sub == "-V" {
            println!("oxiremote {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if sub == "tunnel" {
            // `oxiremote tunnel use <name>`
            let action = argv.get(2).map(String::as_str).unwrap_or("");
            if action == "use" {
                let rest = argv.into_iter().skip(3).collect::<Vec<_>>();
                return tunnel_named::cli_use(rest);
            }
            eprintln!("Usage: oxiremote tunnel use <name>");
            std::process::exit(2);
        }
        if sub == "serve" || sub == "--headless" || sub == "--auto" {
            return run_server_headless();
        }
        if sub == "tui" {
            return run_with_tui();
        }
        if sub == "ui" {
            return run_ui_command();
        }
        if sub == "--tray" {
            // Re-entry from `oxiremote ui` after detached spawn. Server runs
            // on a sibling thread; the tray menu owns the main thread because
            // tray-icon requires it on macOS/Windows.
            return run_with_tray();
        }
        if sub == "--background" {
            // Legacy headless re-entry — kept for older `ui` callers that
            // pre-date the tray integration.
            return run_server_headless();
        }
        if sub == "--help" || sub == "-h" {
            println!(
                "Usage:\n  oxiremote                     Run agent + TUI (if TTY) or headless server\n  oxiremote tui                 Force TUI mode (server in background)\n  oxiremote ui                  Spawn agent in background with menu-bar tray, open browser\n  oxiremote serve               Force headless server mode\n  oxiremote --auto              Headless start (alias of `serve`, useful for Codespaces postStartCommand)\n  oxiremote update              Self-update from the latest GitHub release\n  oxiremote --version           Print version and exit\n  oxiremote notify --title <text> [--body <text>] [--deep-link </h/...>]\n  oxiremote tunnel use <name>   Write ~/.config/oxiremote/tunnel.toml"
            );
            return Ok(());
        }
    }

    // Bare invocation: pick TUI if attached to a terminal, else headless.
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && std::env::var("OXI_HEADLESS").is_err() {
        run_with_tui()
    } else {
        run_server_headless()
    }
}

/// Bind a TCP listener with bounded retry on `AddrInUse`. The TUI parent
/// already binds :8787 to keep the dashboard responsive; when the user picks
/// "Open Web UI" the parent calls `process::exit(0)` and the detached child
/// races to bind before Windows has fully released the parent's listener.
/// On a clean handoff that release window is well under a second, but it can
/// stretch to 1–2s on busy machines. Retry with exponential backoff inside
/// `timeout` so transient cleanup is invisible to the operator; only fall
/// through to the legacy "sweep stale processes" path once the window closes.
async fn bind_with_retry(
    addr: std::net::SocketAddr,
    timeout: std::time::Duration,
) -> std::io::Result<tokio::net::TcpListener> {
    use tokio::time::{Duration, Instant};
    let deadline = Instant::now() + timeout;
    let mut delay_ms = 100u64;
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e)
                if e.kind() == std::io::ErrorKind::AddrInUse
                    && Instant::now() < deadline =>
            {
                tracing::debug!(%addr, delay_ms, "bind AddrInUse — retrying");
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                delay_ms = (delay_ms * 2).min(800);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Poll loopback `AGENT_PORT` with `GET /api/health` until the handler
/// responds 200, or `timeout` elapses. Returns true on first 200, false if
/// the deadline lapses with no successful probe.
///
/// Why HTTP, not raw TCP: `axum::serve` binds the TCP socket *before*
/// `Router::merge` finishes wiring routes, and the Drop on the parent's
/// listener can finish *after* the child has spawned. A pure TCP probe
/// returns true while either of those windows is open, so the caller opens
/// the browser into a "Can't connect" / "Service Unavailable" gap. Hitting
/// `/api/health` confirms (a) the listener is bound, (b) the router knows
/// about the route, and (c) the handler returns. False positives are
/// impossible — only the agent serves "ok" at that path.
pub fn wait_for_agent_ready(timeout: std::time::Duration) -> bool {
    use std::time::{Duration, Instant};

    let url = format!("http://127.0.0.1:{AGENT_PORT}/api/health");
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send()
            && resp.status().is_success()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

/// Env var consumed by `run_with_tray` (and any other server-mode entry):
/// when set, spawn a side thread that polls `/api/health` and opens the
/// system browser on the agent dashboard once the handler responds. Lets
/// the launcher (TUI menu, `oxiremote ui`) hand off the browser-open to
/// the long-lived child instead of racing the parent's exit.
pub const ENV_OPEN_BROWSER_ON_READY: &str = "OXI_OPEN_BROWSER_ON_READY";

/// Spawn a background thread that waits for `/api/health` to respond and
/// then opens the browser on the agent dashboard. Idempotent; safe to call
/// even when `OXI_OPEN_BROWSER_ON_READY` is unset (returns immediately).
fn maybe_open_browser_when_ready() {
    if std::env::var(ENV_OPEN_BROWSER_ON_READY).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("oxiremote-browser-launcher".into())
        .spawn(|| {
            // 30s deadline accommodates first-run cloudflared download.
            if wait_for_agent_ready(std::time::Duration::from_secs(30)) {
                let _ = open::that(format!("http://127.0.0.1:{AGENT_PORT}/agent"));
            }
        })
        .ok();
}

/// `oxiremote ui` — spawn the agent detached and let the child open the
/// browser on `/agent` once its own `/api/health` responds. If a server is
/// already running on the port, open the browser from this process directly
/// (no race — the existing server is already serving).
fn run_ui_command() -> anyhow::Result<()> {
    use std::process::Stdio;
    use std::time::Duration;

    // Probe via /api/health (not raw TCP) so a half-bound listener from a
    // tearing-down sibling process doesn't trip a false positive.
    let already_running = wait_for_agent_ready(Duration::from_millis(750));

    if already_running {
        let agent_root = format!("http://127.0.0.1:{AGENT_PORT}/agent");
        let _ = open::that(&agent_root);
        println!("OxiRemote already running at {agent_root}");
        return Ok(());
    }

    let exe = std::env::current_exe().context("current_exe")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--tray")
        // Hand browser-open off to the child so it fires AFTER the child's
        // own /api/health responds — eliminates the parent-exit race that
        // dropped Safari into "Can't Connect to the Server".
        .env(ENV_OPEN_BROWSER_ON_READY, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        // setsid() detaches from the controlling TTY so the child survives
        // the parent shell exiting and doesn't share signals.
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().context("spawn detached agent")?;
    let pid = child.id();
    let agent_root = format!("http://127.0.0.1:{AGENT_PORT}/agent");
    println!("OxiRemote starting in background (PID: {pid}).");
    println!("  Dashboard will open at {agent_root} once the agent is ready.");
    Ok(())
}

/// Headless path: single tokio runtime on the current (main) thread.
fn run_server_headless() -> anyhow::Result<()> {
    let bus = EventBus::new();
    let data_dir = default_data_dir()?;
    // Hold the file-appender guard for the lifetime of the process so
    // background log lines are flushed at clean shutdown.
    let _log_guard = tracing_setup::init(
        tracing_setup::AgentMode::Headless,
        bus.clone(),
        &data_dir,
    );
    let _lock = instance_lock::InstanceLock::acquire(&data_dir)
        .context("acquire instance lock")?;

    let discovery_url = env_defaults::discovery_url();
    let discovery_temp_key: Arc<StdRwLock<Option<String>>> = Arc::new(StdRwLock::new(None));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async move { server_main(bus, discovery_url, discovery_temp_key).await })
}


/// TUI path: tokio runtime on a sibling thread, TUI event loop on main.
/// The event bus is shared so tunnel/device events drive the TUI live.
///
/// Lazy server spawn: previously this function spawned the server thread
/// (and bound :8787) before the menu was shown. When the user picked
/// "Open Web UI (background)" we then called `process::exit(0)` and let
/// the detached `--tray` child race to bind the same port — on Windows
/// that race regularly cost 5-10s of port-cleanup, breaking the handoff.
/// Now the server thread is only spawned if the user actually picks
/// "Terminal UI", so the "Open Web UI" path becomes byte-equivalent to
/// `oxiremote ui` from a clean state — no race, no bind retry needed.
fn run_with_tui() -> anyhow::Result<()> {
    let bus = EventBus::new();
    let data_dir = default_data_dir()?;
    let _log_guard = tracing_setup::init(
        tracing_setup::AgentMode::Tui,
        bus.clone(),
        &data_dir,
    );
    let lock = instance_lock::InstanceLock::acquire(&data_dir)
        .context("acquire instance lock")?;

    let db_path = data_dir.join("oxiremote.sqlite");

    // Build the discovery handle before either thread starts so server +
    // TUI share the same `discovery_temp_key` slot — the listener writes,
    // the TUI reads, no synchronization beyond the RwLock.
    let discovery_url = env_defaults::discovery_url();
    let discovery_temp_key: Arc<StdRwLock<Option<String>>> = Arc::new(StdRwLock::new(None));

    // Closure that spawns the server thread on demand. The TUI invokes it
    // only when the user enters the Terminal UI dashboard; the "Open Web
    // UI" path never invokes it, so the parent never binds :8787.
    let bus_for_server = bus.clone();
    let discovery_url_srv = discovery_url.clone();
    let discovery_temp_key_srv = discovery_temp_key.clone();
    let spawn_server = move || -> anyhow::Result<()> {
        std::thread::Builder::new()
            .name("oxiremote-server".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        // No stderr in TUI mode — push to bus so log pane sees it.
                        bus_for_server.send(crate::events::AgentEvent::LogEntry {
                            level: crate::events::LogLevel::Error,
                            module: "agent".into(),
                            ts: 0,
                            msg: format!("failed to build tokio runtime: {err}"),
                        });
                        return;
                    }
                };
                if let Err(err) = rt.block_on(server_main(
                    bus_for_server.clone(),
                    discovery_url_srv,
                    discovery_temp_key_srv,
                )) {
                    bus_for_server.send(crate::events::AgentEvent::LogEntry {
                        level: crate::events::LogLevel::Error,
                        module: "agent".into(),
                        ts: 0,
                        msg: format!("server exited: {err:#}"),
                    });
                }
            })
            .context("spawn server thread")?;
        Ok(())
    };

    let result = tui::run_tui(
        bus,
        db_path,
        discovery_url,
        discovery_temp_key,
        spawn_server,
    );
    // Explicit drop — process::exit skips destructors so we'd otherwise leak
    // the PID file and lock out the next start.
    drop(lock);
    match result {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("tui error: {err:#}");
            std::process::exit(1);
        }
    }
}

/// Tray path: tokio runtime on a sibling thread, tray-icon event loop on
/// main. macOS / Windows require the tray to live on the main thread, so the
/// dispatcher is symmetric with `run_with_tui` — only the consumer differs.
fn run_with_tray() -> anyhow::Result<()> {
    // Windows: the parent (`oxiremote ui` or the TUI's "Open Web UI") spawned
    // us with DETACHED_PROCESS, so we have no console. Cloudflared is a CUI
    // subsystem child and on some Windows configurations refuses to function
    // without a real console handle (it bails on its first quick-tunnel API
    // call with "invalid UUID length: 0"). Allocate our own console up front
    // and immediately hide its window so the user never sees it. Cloudflared
    // then inherits a valid console handle when we spawn it later.
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Console::{AllocConsole, GetConsoleWindow};
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        // Best-effort. AllocConsole returns 0 if a console is already
        // attached (e.g. agent launched directly from PowerShell without
        // detach) — that's fine; we just hide whatever we have.
        let _ = AllocConsole();
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    let bus = EventBus::new();
    let data_dir = default_data_dir()?;
    // File-log guard MUST live until the tray loop exits; we hand it to a
    // local binding so it's dropped on `process::exit(0)` from the Shutdown
    // menu (drop = flush + close the appender thread).
    let _log_guard = tracing_setup::init(
        tracing_setup::AgentMode::Headless,
        bus.clone(),
        &data_dir,
    );
    let _lock = instance_lock::InstanceLock::acquire(&data_dir)
        .context("acquire instance lock")?;

    let discovery_url = env_defaults::discovery_url();
    let discovery_temp_key: Arc<StdRwLock<Option<String>>> = Arc::new(StdRwLock::new(None));

    // Allocate the tray-cmd channel up-front so we can move the Sender into
    // the server thread (which owns the only tokio runtime in this process)
    // and keep the Receiver on the main thread for the tray event loop.
    // Spawning the bridge directly via `tokio::spawn` from the main thread
    // would panic with "no reactor running" — there is no runtime here.
    let (tray_cmd_tx, tray_cmd_rx) = tray::tray_cmd_channel();

    let bus_for_server = bus.clone();
    let discovery_url_srv = discovery_url.clone();
    let discovery_temp_key_srv = discovery_temp_key.clone();
    std::thread::Builder::new()
        .name("oxiremote-server".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    bus_for_server.send(crate::events::AgentEvent::LogEntry {
                        level: crate::events::LogLevel::Error,
                        module: "agent".into(),
                        ts: 0,
                        msg: format!("failed to build tokio runtime: {err}"),
                    });
                    return;
                }
            };
            // Bridge tunnel events to the main-thread tray loop. Spawned on
            // this runtime BEFORE block_on so the watcher lives the full
            // server lifetime and shares the same broadcast bus.
            rt.spawn(tray::run_degraded_watcher(
                bus_for_server.clone(),
                tray_cmd_tx,
            ));
            if let Err(err) = rt.block_on(server_main(
                bus_for_server.clone(),
                discovery_url_srv,
                discovery_temp_key_srv,
            )) {
                bus_for_server.send(crate::events::AgentEvent::LogEntry {
                    level: crate::events::LogLevel::Error,
                    module: "agent".into(),
                    ts: 0,
                    msg: format!("server exited: {err:#}"),
                });
            }
        })
        .context("spawn server thread")?;

    drop(bus); // bus stays live via the server thread's clone; we don't need ours

    // If launched with `OXI_OPEN_BROWSER_ON_READY=1` (TUI menu hand-off or
    // `oxiremote ui`), poll our own /api/health and open the browser when it
    // responds. Doing this from the child — not the launcher — closes the
    // race where the launcher's TCP probe of the *parent's* listener
    // returned true, the launcher opened the browser, the launcher exited
    // (port released), and Safari hit "Can't Connect to the Server" before
    // this child finished binding.
    maybe_open_browser_when_ready();

    // Build the tray on the main thread, then drive its event loop. The loop
    // never returns under normal use — `Shutdown` from the menu calls
    // `process::exit(0)` directly so the lock drop is unreachable here.
    let handle = tray::build_tray(tray_cmd_rx).context("build tray")?;
    tray::run_event_loop(&handle);
    unreachable!()
}

async fn server_main(
    event_bus: Arc<EventBus>,
    discovery_url: Option<String>,
    discovery_temp_key: Arc<StdRwLock<Option<String>>>,
) -> anyhow::Result<()> {
    let data_dir = default_data_dir()?;
    std::fs::create_dir_all(&data_dir).context("create data dir")?;

    let cloudflared = tunnel::ensure_cloudflared(&data_dir, event_bus.clone())
        .await
        .context("ensure cloudflared")?;
    info!(path = %cloudflared.display(), "cloudflared ready");

    let db_path = data_dir.join("oxiremote.sqlite");
    db::init_db(&db_path).context("init db")?;

    // Mark stale `running` rows as `dead` — their PTY processes died with the
    // previous agent. Idempotent; safe on every boot.
    match terminal_api::reconcile_orphan_sessions(&db_path) {
        Ok(n) if n > 0 => info!(count = n, "reconciled orphan terminal sessions"),
        Ok(_) => {}
        Err(err) => warn!(error=%err, "orphan reconciliation failed"),
    }

    let key_path = data_dir.join("signing.key");
    let signing_key = auth::load_or_create_key(&key_path).context("load signing key")?;

    // Derive + persist stable host identity (hostname + install salt → blake3 hash).
    let host_info = {
        let conn = rusqlite::Connection::open(&db_path).context("open db for host init")?;
        let info = host::ensure_host(&data_dir, &conn).context("ensure host")?;
        workspaces::seed_defaults(&conn, &info.host_id).context("seed workspaces")?;
        info
    };

    // Resolved via env_defaults: defaults to true (OXI_SECURE_COOKIES=1) when
    // the var is unset. Empty value (OXI_SECURE_COOKIES=) → false (opt-out for
    // self-hosted HTTP-only deployments).
    let secure_cookies = env_defaults::secure_cookies();

    let workspace_root = std::env::var("OXI_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let vapid_keys = Arc::new(push::load_or_create_vapid(&data_dir).context("init vapid")?);
    let notify_token = push::load_or_create_notify_token(&data_dir).context("init notify token")?;
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("http client")?;

    // Reused by the /preview proxy — redirects stay client-side, bypass any
    // system proxy, pool connections across requests.
    let preview_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("preview http client")?;

    // Restore persisted previews into the in-memory cache (lookup is hot path).
    let preview_targets: DashMap<String, PreviewTarget> = DashMap::new();
    match preview::load_all_previews(&db_path, &host_info.host_id) {
        Ok(rows) => {
            for t in rows {
                preview_targets.insert(t.id.clone(), t);
            }
            info!(count = preview_targets.len(), "previews restored");
        }
        Err(err) => warn!(error=%err, "preview restore failed; starting empty"),
    }

    let local_sites_cache = local_sites::new_cache();

    let proxy_allowed_ports: HashSet<u16> = match db::load_proxy_allowed_ports(&db_path) {
        Ok(ports) => ports.into_iter().collect(),
        Err(err) => {
            warn!(error=%err, "failed to load proxy_allowed_ports; defaulting to empty");
            HashSet::new()
        }
    };
    let proxy_allowed_ports = Arc::new(StdRwLock::new(proxy_allowed_ports));

    // CORS allowlist: env var (backward compat) ∪ persisted DB origins.
    // env_defaults::cors_origins() supplies the bundled default (remote.erai.dev)
    // when OXI_CORS_ORIGINS is unset; empty value = explicit opt-out.
    // Mutable at runtime via pairing handlers (auto-add) and /api/agent/cors.
    let cors_db_origins = db::load_cors_allowed_origins(&db_path).unwrap_or_else(|err| {
        warn!(error=%err, "failed to load cors_allowed_origins; defaulting to empty");
        Vec::new()
    });
    let cors_env_origins = security::cors::resolved_origins();
    let cors_origins = security::cors::seed_origins(&cors_env_origins, &cors_db_origins);

    // Probe desktop availability once at boot. On macOS this triggers the TCC
    // Screen Recording prompt on first run — expected behaviour. The probe runs
    // on a blocking thread so the Linux PipeWire D-Bus handshake (up to 3s,
    // internally timeout-capped) cannot stall the axum serve loop.
    #[cfg(feature = "desktop")]
    let desktop_avail = {
        let avail = tokio::task::spawn_blocking(desktop::desktop_available)
            .await
            .unwrap_or(false);
        info!(available = avail, "desktop capture probe");
        avail
    };
    #[cfg(not(feature = "desktop"))]
    let desktop_avail = false;

    // Build desktop service if capture is available. This is a lightweight
    // DashMap registry — no background tasks are spawned here.
    #[cfg(feature = "desktop")]
    let desktop_svc: Option<Arc<DesktopService>> = if desktop_avail {
        Some(Arc::new(DesktopService::new()))
    } else {
        None
    };
    #[cfg(not(feature = "desktop"))]
    let desktop_svc: Option<()> = None;

    let telemetry = Arc::new(TelemetryState::new());
    let files_activity = Arc::new(files_activity::new_map());

    // Read tunnel config early so `is_quick_tunnel` is available for AppState.
    // Stored in a separate variable; the actual supervisor spawn uses the same
    // value below.
    let named_cfg_for_state = tunnel_named::load().unwrap_or(None);
    let is_quick_tunnel = named_cfg_for_state.is_none();
    let named_tunnel_hostname = named_cfg_for_state
        .as_ref()
        .and_then(|c| c.hostname.clone());

    let state = Arc::new(AppState {
        db_path,
        signing_key,
        secure_cookies,
        terminal_sessions: Arc::new(DashMap::new()),
        preview_targets,
        preview_health: DashMap::new(),
        local_sites: local_sites_cache.clone(),
        proxy_allowed_ports,
        pairing_attempts: DashMap::new(),
        workspace_root,
        host_info,
        vapid_keys,
        notify_token,
        http_client,
        preview_client,
        rate_limiter: Arc::new(RateLimiter::new()),
        event_bus,
        tunnel_url: Arc::new(std::sync::RwLock::new(None)),
        latest_tunnel_step: Arc::new(StdRwLock::new(None)),
        recent_logs: Arc::new(StdMutex::new(VecDeque::with_capacity(LOG_RING_CAP))),
        desktop_available: desktop_avail,
        desktop_service: desktop_svc,
        discovery_url: discovery_url.clone(),
        web_url: env_defaults::web_url(),
        is_quick_tunnel,
        named_tunnel_hostname,
        discovery_temp_key: discovery_temp_key.clone(),
        tunnel_shutdown: Arc::new(tokio::sync::Notify::new()),
        telemetry,
        files_activity,
        cloudflared_path: Some(cloudflared.clone()),
        cors_origins: cors_origins.clone(),
    });

    // Background: periodic listening-port discovery + preview health checks.
    local_sites::spawn_discovery_loop(local_sites_cache);
    preview::spawn_health_loop(state.clone());

    // Snapshot maintainer — single bus subscriber that mirrors the latest
    // `TunnelStepChanged`, the last `LOG_RING_CAP` `LogEntry` events, and
    // telemetry tunnel-ready timing into AppState. SSE late-joiners hydrate
    // from `/api/agent/state` and `/api/agent/logs/recent` instead of
    // starting empty.
    {
        let snap_state = state.clone();
        let mut rx = state.event_bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => match &ev {
                        AgentEvent::TunnelStepChanged { step, .. } => {
                            if let Ok(mut g) = snap_state.latest_tunnel_step.write() {
                                *g = Some(ev.clone());
                            }
                            // Mark tunnel ready for uptime + reconnect tracking.
                            if matches!(step, crate::events::TunnelStep::Ready) {
                                snap_state.telemetry.mark_tunnel_ready();
                            }
                        }
                        AgentEvent::HealthProbe { elapsed_ms, ok, .. } => {
                            // Record every probe sample (ok or not) for p50 sparkline.
                            if *ok {
                                snap_state.telemetry.record_probe_latency(*elapsed_ms);
                            }
                        }
                        AgentEvent::LogEntry { .. } => {
                            if let Ok(mut g) = snap_state.recent_logs.lock() {
                                if g.len() >= LOG_RING_CAP {
                                    g.pop_front();
                                }
                                g.push_back(ev.clone());
                            }
                        }
                        _ => {}
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Slow consumer fell behind — keep going. Tunnel step
                        // emits at most ~10 events per session so this is
                        // dominated by log spikes; missing a few logs is OK.
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // Discovery client — when `OXI_DISCOVERY_URL` is set, subscribe to the bus
    // and re-register with the worker on every `TunnelUrlChanged`. Subscribed
    // BEFORE the tunnel task spawns below so the first URL emit is not missed
    // (tokio broadcast has no replay).
    if let Some(url) = discovery_url.clone() {
        match discovery::load_discovery_id(&state.db_path) {
            Ok(discovery_id) => {
                // Heartbeat: refreshes the worker's session record TTL. The
                // tunnel.rs stderr parser also re-emits TunnelUrlChanged on
                // mid-process URL rotations, but rotations aren't guaranteed
                // — without this loop a long-lived stable tunnel's session
                // row would fall out of KV (24h TTL) and every cross-origin
                // OTK / code / sk- lookup would 404 even with a healthy
                // tunnel. Re-uses the live tunnel_url snapshot.
                {
                    let hb_client = state.http_client.clone();
                    let hb_url = url.clone();
                    let hb_discovery_id = discovery_id.clone();
                    let hb_tunnel = state.tunnel_url.clone();
                    tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(
                            std::time::Duration::from_secs(discovery::HEARTBEAT_INTERVAL_SECS),
                        );
                        // Skip the immediate first tick; the TunnelUrlChanged
                        // listener already does the initial registration.
                        ticker.tick().await;
                        loop {
                            ticker.tick().await;
                            let snapshot = hb_tunnel.read().ok().and_then(|g| g.clone());
                            if let Some(tu) = snapshot {
                                discovery::spawn_refresh_session(
                                    hb_client.clone(),
                                    hb_url.clone(),
                                    hb_discovery_id.clone(),
                                    tu,
                                );
                            }
                        }
                    });
                }
                let bus = state.event_bus.clone();
                let client = state.http_client.clone();
                let slot = discovery_temp_key.clone();
                let db_path = state.db_path.clone();
                let mut rx = state.event_bus.subscribe();
                tokio::spawn(async move {
                    // Track the most recent tunnel URL so we can register it
                    // when the probe finally promotes to Ready. Phase 4: don't
                    // tell the worker about a URL we know is bad — wait for
                    // the dashboard's own probe to clear it. The same Ready
                    // event drives every URL rotation, so a transient
                    // Degraded → Ready cycle re-registers the new URL.
                    let mut latest_url: Option<String> = None;
                    loop {
                        match rx.recv().await {
                            Ok(AgentEvent::TunnelUrlChanged { url: tunnel_url }) => {
                                latest_url = Some(tunnel_url);
                            }
                            Ok(AgentEvent::TunnelStepChanged {
                                step: events::TunnelStep::Ready,
                                ..
                            }) => {
                                let Some(tunnel_url) = latest_url.clone() else {
                                    continue;
                                };
                                // Read the active pairing code per-event — the agent rotates
                                // it every 5 min, and we want the latest valid one each time
                                // we re-publish (e.g. on tunnel reconnect).
                                let code = discovery::active_pairing_code(&db_path)
                                    .ok()
                                    .flatten();
                                // Re-register the permanent-key lookup_id alongside so cross-
                                // origin sk-… pairing keeps resolving against the latest tunnel.
                                let lookup_id =
                                    crate::auth::get_permanent_key_lookup_id(&db_path)
                                        .ok()
                                        .flatten();
                                discovery::spawn_register(
                                    client.clone(),
                                    url.clone(),
                                    discovery_id.clone(),
                                    tunnel_url,
                                    slot.clone(),
                                    bus.clone(),
                                    code,
                                    lookup_id,
                                );
                            }
                            Ok(AgentEvent::TunnelStepChanged {
                                step: events::TunnelStep::Degraded,
                                reason,
                                ..
                            }) => {
                                warn!(
                                    target: "discovery",
                                    reason = ?reason,
                                    "tunnel degraded — skipping discovery registration"
                                );
                            }
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });
                info!("discovery client enabled (worker URL configured)");
            }
            Err(err) => {
                warn!(error=%err, "discovery_id load failed; discovery client disabled");
            }
        }
    }

    // Keep state.tunnel_url in sync with TunnelUrlChanged events. cloudflared
    // can rotate the public URL mid-process when Cloudflare invalidates a
    // Quick Tunnel session (edge migration, network blip). The tunnel.rs
    // stderr parser detects this and re-emits TunnelUrlChanged; this listener
    // propagates it into the shared slot so the heartbeat (reads
    // tunnel_url.read()) and any future readers see the fresh value without
    // requiring an agent restart.
    {
        let slot = state.tunnel_url.clone();
        let mut rx = state.event_bus.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let AgentEvent::TunnelUrlChanged { url } = ev
                    && let Ok(mut guard) = slot.write()
                {
                    *guard = Some(url);
                }
            }
        });
    }

    // Desktop notifications + agent-end Web Push. Tray runtime is not yet
    // wired; device-pending toasts and agent-finish pushes work in all modes.
    {
        // VAPID subject — mailto or https origin identifying the push sender.
        let notify_subject = format!("mailto:{}", std::env::var("OXI_PUSH_SUBJECT")
            .unwrap_or_else(|_| "admin@localhost".into()));
        notifier::spawn_event_notifier(
            state.event_bus.clone(),
            state.db_path.clone(),
            state.host_info.host_id.clone(),
            state.vapid_keys.clone(),
            state.http_client.clone(),
            notify_subject,
        );
    }

    let app = Router::new()
        .route("/api/health", get(api_health))
        .route("/api/me", get(http_pages::api_me))
        .route("/api/pairing/exchange", post(http_pages::api_pairing_exchange))
        .route("/api/login/one-time", post(http_pages::api_login_one_time))
        .route("/api/auth/approval-status", get(http_pages::api_auth_approval_status))
        .route("/api/auth/logout", post(http_pages::api_logout))
        .route("/api/devices", get(http_pages::api_devices_list))
        .route("/api/devices/{id}/revoke", post(http_pages::api_device_revoke))
        // host
        .merge(host_api::router())
        // agent-local dashboard + event bus (localhost only, enforced by route_scope)
        .merge(agent_api::router())
        // push + notify
        .merge(push_api::router())
        // terminal
        .route(
            "/api/terminal/sessions",
            get(terminal_api::api_terminal_sessions_list).post(terminal_api::api_terminal_sessions_create),
        )
        .route("/api/terminal/sessions/{id}/ws", get(terminal_ws::api_terminal_session_ws))
        .route("/api/terminal/sessions/{id}/resize", post(terminal_api::api_terminal_session_resize))
        // desktop WebRTC + WS fallback (feature-gated; 503 when unavailable)
        .route("/ws/desktop/{device_id}", get({
            #[cfg(feature = "desktop")]
            { desktop_ws::api_desktop_ws }
            #[cfg(not(feature = "desktop"))]
            { || async { axum::http::StatusCode::SERVICE_UNAVAILABLE } }
        }))
        .route("/api/terminal/sessions/{id}/close", post(terminal_api::api_terminal_session_close))
        .route("/api/terminal/sessions/{id}/notify-on-finish", post(terminal_api::api_terminal_session_notify_on_finish))
        .route("/api/terminal/sessions/{id}", axum::routing::patch(terminal_api::api_terminal_session_rename))
        // git
        .route("/api/git/status", get(git::api_git_status))
        .route("/api/git/diff", get(git::api_git_diff))
        .route("/api/git/stage", post(git::api_git_stage))
        .route("/api/git/unstage", post(git::api_git_unstage))
        .route("/api/git/commit", post(git::api_git_commit))
        // files
        .route("/api/files/list", get(files::api_files_list))
        .route("/api/files/search", get(files_search::api_files_search))
        .route("/api/files/stat", get(files::api_files_stat))
        .route("/api/files/read", get(files::api_files_read))
        .route("/api/files/write", post(files::api_files_write))
        .route("/api/files/create", post(files::api_files_create))
        .route("/api/files/rename", post(files::api_files_rename))
        .route("/api/files/delete", post(files::api_files_delete))
        .route("/api/files/download", get(files::api_files_download))
        .route(
            "/api/files/upload",
            post(files_upload::api_files_upload)
                .layer(DefaultBodyLimit::max(files_upload::MAX_UPLOAD_BYTES as usize)),
        )
        // workspaces
        .route(
            "/api/workspaces",
            get(workspaces::api_workspaces_list).post(workspaces::api_workspaces_create),
        )
        .route(
            "/api/workspaces/{id}",
            axum::routing::delete(workspaces::api_workspaces_delete),
        )
        .route(
            "/api/workspaces/{id}/touch",
            post(workspaces::api_workspaces_touch),
        )
        .route(
            "/api/workspace/validate",
            post(workspaces::api_workspace_validate),
        )
        // preview proxy
        .route("/api/previews", get(preview::api_previews_list).post(preview::api_previews_create))
        .route("/api/previews/{id}", axum::routing::delete(preview::api_previews_delete))
        .route("/api/previews/{id}/share", post(preview::api_previews_share))
        .route("/api/local-sites", get(local_sites::api_local_sites))
        .route("/preview/{id}", axum::routing::any(preview::preview_proxy_root_handler))
        .route("/preview/{id}/{*rest}", axum::routing::any(preview::preview_proxy_handler))
        // Local-sites reverse proxy. Default-deny allowlist is checked inside
        // the handler; bare `/proxy/<port>` redirects to the trailing-slash
        // form so relative paths in upstream HTML resolve.
        .route("/proxy/{port}", axum::routing::any(proxy::proxy_root_redirect))
        .route("/proxy/{port}/", axum::routing::any(proxy::proxy_root_handler))
        .route("/proxy/{port}/{*rest}", axum::routing::any(proxy::proxy_handler));

    // Dev keeps the server-rendered `/` and `/login` for the no-SPA pairing
    // bootstrap. They take routing precedence over the SPA fallback below.
    #[cfg(debug_assertions)]
    let app = app
        .route("/", get(http_pages::app_root))
        .route("/login", get(http_pages::login_page));

    // SPA fallback — serves the embedded web assets in release, and reads
    // from `apps/web/dist/` at runtime in debug (rust-embed default). When
    // `dist/` is missing in debug, the fallback renders a help page pointing
    // the user at `bun run build:web` or `bun dev`.
    let app = app.fallback(static_files::spa_handler);

    // Middleware order matters: from outermost → innermost request:
    //   1. tunnel_guard rejects Localhost-only routes over the tunnel (403)
    //   2. rate_limit throttles tunnel callers per (session, route_class)
    //   3. csrf_guard validates header/cookie on state-changing tunnel POSTs
    //   4. api_key_guard requires Bearer on non-exempt tunnel routes
    //   5. bytes_counter accumulates tunnel 2xx response body bytes (innermost)
    // Axum's `.layer()` applies in REVERSE order of declaration, so we reverse
    // the list visually here.
    let rate_limiter = state.rate_limiter.clone();
    let telemetry_for_middleware = state.telemetry.clone();
    let app = app
        .layer(axum::middleware::from_fn_with_state(
            telemetry_for_middleware,
            security::bytes_counter::bytes_counter,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::api_key_guard::api_key_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::csrf::csrf_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            rate_limiter,
            security::rate_limit::rate_limit,
        ))
        .layer(axum::middleware::from_fn(security::tunnel_guard))
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http());

    // CORS sits OUTSIDE the security chain so OPTIONS preflights — which
    // arrive without Bearer / cookie / CSRF — get an early answer instead of
    // hitting `tunnel_guard` / `api_key_guard`. The runtime allowlist is empty
    // by default; same-origin requests carry no `Origin` header so the layer
    // is invisible to embedded same-host flows.
    {
        let count = cors_origins.read().map(|s| s.len()).unwrap_or(0);
        info!(origins = count, "CORS runtime allowlist installed");
    }
    let app = app.layer(security::cors::CorsLayer::new(cors_origins.clone()));

    let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], AGENT_PORT));
    // The tunnel-origin detection in route_scope relies on the agent binding
    // loopback-only. If this assert fires the threat model breaks.
    debug_assert!(addr.ip().is_loopback(), "agent must bind loopback only");
    info!(%addr, "starting agent server");

    // Dev affordance: tell the operator which URL to open. If the SPA has
    // been built we serve it directly from :8787; otherwise we point them at
    // `bun dev` (Vite on :5173) or `bun run build:web` for standalone.
    #[cfg(debug_assertions)]
    {
        let dist_index = std::path::Path::new("apps/web/dist/index.html");
        let dist_index_alt = std::path::Path::new("../apps/web/dist/index.html");
        if dist_index.exists() || dist_index_alt.exists() {
            info!(
                "dev: SPA built — open http://localhost:{} in your browser",
                AGENT_PORT
            );
        } else {
            info!(
                "dev: SPA not built — run `bun run build:web` then refresh, or `bun dev` for hot-reload at http://localhost:5173"
            );
        }
    }

    let listener = match bind_with_retry(addr, std::time::Duration::from_secs(5)).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // The retry window expired with the port still in use. Either a
            // genuine orphan oxiremote is holding it (prior crash without
            // Drop, pre-0.1.1 data_dir bug on Windows) or some unrelated app
            // grabbed :8787. Sweep the process table — if anything was
            // killed, give it a moment and try once more before giving up.
            tracing::warn!(%addr, "port in use after retry window — sweeping stale oxiremote processes");
            let killed = instance_lock::kill_other_oxiremote_processes();
            if killed == 0 {
                eprintln!(
                    "\nError: port {AGENT_PORT} is in use and no stale oxiremote process was found.\n\
                     Another application is bound to 127.0.0.1:{AGENT_PORT}. Free the port and retry."
                );
                return Err(e.into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            tokio::net::TcpListener::bind(addr).await.with_context(|| {
                format!(
                    "rebind after killing {killed} stale oxiremote process(es) failed"
                )
            })?
        }
        Err(e) => return Err(e.into()),
    };
    notifier::show_startup(addr);

    let pairing = http_pages::create_pairing_code(&state).context("create pairing code")?;
    info!(pairing_code = %pairing.code, "pair to continue");

    // Shared signal: heartbeat notifies this when a post-wake probe fails.
    // Supervisor's inner select reacts by killing and respawning cloudflared.
    let force_respawn = Arc::new(tokio::sync::Notify::new());

    // Start tunnel supervisor in background — respawns cloudflared automatically
    // on TunnelDown or heartbeat-triggered force_respawn without restarting the
    // agent process. If `~/.config/oxiremote/tunnel.toml` exists, run a named
    // tunnel; else fall back to a Quick Tunnel for the dev/first-run experience.
    // (`named_cfg_for_state` was read earlier to populate `AppState::is_quick_tunnel`.)
    let tunnel_state = state.clone();
    let tunnel_shutdown = state.tunnel_shutdown.clone();
    let force_respawn_sup = force_respawn.clone();
    tokio::spawn(async move {
        let mode = match named_cfg_for_state {
            Some(cfg) => tunnel_supervisor::TunnelMode::Named(cfg),
            None => tunnel_supervisor::TunnelMode::Quick(addr),
        };

        // The supervisor emits TunnelUrlChanged on each successful spawn;
        // the health-probe and Ready step logic runs inside a subscriber task
        // below that reacts to that event.
        if let Err(err) = tunnel_supervisor::run(
            mode,
            cloudflared,
            tunnel_state.event_bus.clone(),
            tunnel_shutdown,
            force_respawn_sup,
        )
        .await
        {
            warn!(error=%err, "tunnel supervisor exited with error");
        }
    });

    // Heartbeat: detects sleep/wake via wall vs monotonic clock skew, then
    // probes the local agent AND the public tunnel URL. On either probe
    // failing, signals force_respawn so the supervisor kills and respawns
    // cloudflared. Spawned at agent startup so it covers the window between
    // agent boot and first tunnel ready.
    heartbeat::spawn(
        addr,
        state.event_bus.clone(),
        force_respawn.clone(),
        state.tunnel_shutdown.clone(),
    );

    // Long-running edge-health monitor: 30s HEAD probes against the public
    // tunnel URL after first Ready; 3 consecutive failures → force_respawn.
    // Catches the cloudflared "stuck retrying control stream" zombie state
    // that the heartbeat-on-skew check can miss when no sleep/wake happened.
    edge_health_monitor::spawn(
        state.event_bus.clone(),
        force_respawn,
        state.tunnel_shutdown.clone(),
    );

    // Health-probe task: fires on every TunnelUrlChanged (first spawn + every
    // respawn). Runs Verifying probes then emits Ready.
    {
        let probe_state = state.clone();
        let mut probe_rx = state.event_bus.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = probe_rx.recv().await {
                let u = match ev {
                    events::AgentEvent::TunnelUrlChanged { url } => url,
                    _ => continue,
                };

                // Step 4 — tunnel transport up; begin health probes.
                probe_state.event_bus.send(events::AgentEvent::TunnelStepChanged {
                    step: events::TunnelStep::Verifying,
                    attempt: 1,
                    info: Some("running HTTP health probes…".into()),
                    reason: None,
                });

                // Race the active probe loop against the first real client event.
                // The probe uses the system DNS resolver, which can lag for a
                // freshly-issued `*.trycloudflare.com` subdomain. Real clients
                // resolve via Cloudflare's edge and may succeed long before the
                // local resolver catches up — when that happens, treating "client
                // got through" as Ready avoids a spurious 3-minute timeout.
                let probe_bus = probe_state.event_bus.clone();
                let probe_url = u.clone();
                let probe_client = probe_state.http_client.clone();
                let probe_fut = async move {
                    health_check::run_health_check(probe_url, probe_bus, probe_client).await
                };

                let mut client_rx = probe_state.event_bus.subscribe();
                let first_client_fut = async move {
                    loop {
                        match client_rx.recv().await {
                            Ok(events::AgentEvent::OtkUsed { .. })
                            | Ok(events::AgentEvent::DevicePending { .. })
                            | Ok(events::AgentEvent::DeviceApproved { .. })
                            | Ok(events::AgentEvent::DeviceConnected { .. }) => return true,
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => return false,
                        }
                    }
                };

                // A real client connecting through the tunnel is the strongest
                // possible liveness signal — promotes to Ready with no soft
                // suffix even if the local probe is still grinding.
                let outcome = tokio::select! {
                    h = probe_fut => h,
                    client_won = first_client_fut => {
                        if client_won {
                            health_check::ProbeOutcome::Ok
                        } else {
                            health_check::ProbeOutcome::Network("subscriber closed".into())
                        }
                    }
                };

                let next_step = probe_outcome_to_step(&outcome, &u);
                probe_state.event_bus.send(next_step);
            }
        });
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
    // Wake any parked main thread (background mode) so the process can exit
    // cleanly. The server thread will return from axum::serve right after.
    crate::tui::wake_background_main();
}

async fn api_health() -> &'static str {
    "ok"
}

/// Map a verifying-loop probe outcome to the right `TunnelStepChanged` event.
/// Phase 4: the dashboard gets honest state instead of soft-Ready-everything.
///
/// - `Ok` → green Ready (no suffix).
/// - `Timeout` → amber-Ready with the existing soft-fallback suffix. Local
///   DNS / PoP latency is the most common cause and cellular phones via
///   Cloudflare's edge resolver still pair fine; don't paint the dashboard
///   red over a slow probe.
/// - `DohNxdomain` / `HttpError(5xx)` / `Network` → red `Degraded` with the
///   reason populated so the operator can see what broke.
/// - `HttpError(4xx)` other than 401/403 → green Ready: the agent answered,
///   we just hit a route the proxy doesn't recognise.
fn probe_outcome_to_step(
    outcome: &health_check::ProbeOutcome,
    tunnel_url: &str,
) -> events::AgentEvent {
    use health_check::ProbeOutcome;
    match outcome {
        ProbeOutcome::Ok => events::AgentEvent::TunnelStepChanged {
            step: events::TunnelStep::Ready,
            attempt: 1,
            info: Some(tunnel_url.to_string()),
            reason: None,
        },
        ProbeOutcome::Timeout => events::AgentEvent::TunnelStepChanged {
            step: events::TunnelStep::Ready,
            attempt: 1,
            info: Some(format!(
                "{tunnel_url} (local probe inconclusive — scan QR to test)"
            )),
            reason: None,
        },
        ProbeOutcome::HttpError(code) if (400..500).contains(code) && *code != 401 && *code != 403 => {
            events::AgentEvent::TunnelStepChanged {
                step: events::TunnelStep::Ready,
                attempt: 1,
                info: Some(tunnel_url.to_string()),
                reason: None,
            }
        }
        ProbeOutcome::DohNxdomain => events::AgentEvent::TunnelStepChanged {
            step: events::TunnelStep::Degraded,
            attempt: 1,
            info: Some(tunnel_url.to_string()),
            reason: Some("tunnel hostname not registered with Cloudflare".into()),
        },
        ProbeOutcome::HttpError(code) => events::AgentEvent::TunnelStepChanged {
            step: events::TunnelStep::Degraded,
            attempt: 1,
            info: Some(tunnel_url.to_string()),
            reason: Some(format!("tunnel returned HTTP {code}")),
        },
        ProbeOutcome::Network(msg) => events::AgentEvent::TunnelStepChanged {
            step: events::TunnelStep::Degraded,
            attempt: 1,
            info: Some(tunnel_url.to_string()),
            reason: Some(format!("tunnel network error: {msg}")),
        },
    }
}

#[cfg(test)]
mod probe_outcome_to_step_tests {
    use super::*;
    use health_check::ProbeOutcome;

    fn assert_step(outcome: ProbeOutcome, expected: events::TunnelStep, expect_reason: bool) {
        let ev = probe_outcome_to_step(&outcome, "https://x.trycloudflare.com");
        let events::AgentEvent::TunnelStepChanged { step, reason, .. } = ev else {
            panic!("expected TunnelStepChanged");
        };
        assert_eq!(step, expected);
        assert_eq!(reason.is_some(), expect_reason);
    }

    #[test]
    fn ok_maps_to_ready_no_reason() {
        assert_step(ProbeOutcome::Ok, events::TunnelStep::Ready, false);
    }

    #[test]
    fn timeout_maps_to_ready_with_soft_suffix() {
        let ev = probe_outcome_to_step(&ProbeOutcome::Timeout, "https://x.trycloudflare.com");
        let events::AgentEvent::TunnelStepChanged { step, info, .. } = ev else {
            panic!("expected TunnelStepChanged");
        };
        assert_eq!(step, events::TunnelStep::Ready);
        assert!(info.unwrap().contains("local probe inconclusive"));
    }

    #[test]
    fn nxdomain_maps_to_degraded_with_reason() {
        assert_step(ProbeOutcome::DohNxdomain, events::TunnelStep::Degraded, true);
    }

    #[test]
    fn http_5xx_maps_to_degraded() {
        assert_step(ProbeOutcome::HttpError(503), events::TunnelStep::Degraded, true);
    }

    #[test]
    fn http_404_maps_to_ready() {
        assert_step(ProbeOutcome::HttpError(404), events::TunnelStep::Ready, false);
    }

    #[test]
    fn http_401_maps_to_degraded() {
        // 401/403 stay in the Degraded bucket — they suggest auth misconfig
        // rather than route mismatch and should be visible to the operator.
        assert_step(ProbeOutcome::HttpError(401), events::TunnelStep::Degraded, true);
    }

    #[test]
    fn network_error_maps_to_degraded() {
        assert_step(
            ProbeOutcome::Network("connection refused".into()),
            events::TunnelStep::Degraded,
            true,
        );
    }
}
