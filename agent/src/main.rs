mod agent_api;
mod approval;
mod auth;
mod db;
mod events;
mod files;
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
mod tracing_setup;
mod update;
mod static_files;
#[cfg(feature = "desktop")]
mod desktop_service;
#[cfg(feature = "desktop")]
mod desktop_ws;
#[cfg(feature = "desktop")]
mod desktop_ws_capture;
// Phase 03 pipeline selection — operator env flag + client capability AND.
#[cfg(feature = "desktop")]
mod pipeline_selection;
// Phase 03 H.264 pipeline — gated so the JPEG default build doesn't depend
// on the VT/OpenH264 toolchain.
#[cfg(feature = "h264")]
mod video_pipeline;
mod terminal_api;
mod terminal_buffer;
mod terminal_pty;
mod terminal_ws;
mod tray;
mod tui;
mod tunnel;
mod tunnel_named;
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
use crate::host::HostInfo;
use crate::local_sites::LocalSitesCache;
use crate::preview::{PreviewHealth, PreviewTarget};
use crate::push::VapidKeys;
use crate::security::rate_limit::RateLimiter;
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
    pub terminal_sessions: DashMap<String, Arc<TerminalSession>>,
    pub preview_targets: DashMap<String, PreviewTarget>,
    pub preview_health: DashMap<String, PreviewHealth>,
    pub local_sites: LocalSitesCache,
    /// Phase 02 — set of localhost ports operators have opted into for the
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
}

fn default_data_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
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
            return update::run();
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
        if sub == "--tray" || sub == "--background" {
            // Re-entry from `oxiremote ui` after detached spawn — same as headless,
            // but no log output to stderr (parent already detached our stdio).
            return run_server_headless();
        }
        if sub == "--help" || sub == "-h" {
            println!(
                "Usage:\n  oxiremote                     Run agent + TUI (if TTY) or headless server\n  oxiremote tui                 Force TUI mode (server in background)\n  oxiremote ui                  Spawn agent in background, open browser to dashboard\n  oxiremote serve               Force headless server mode\n  oxiremote --auto              Headless start (alias of `serve`, useful for Codespaces postStartCommand)\n  oxiremote update              Self-update from the latest GitHub release\n  oxiremote --version           Print version and exit\n  oxiremote notify --title <text> [--body <text>] [--deep-link </h/...>]\n  oxiremote tunnel use <name>   Write ~/.config/oxiremote/tunnel.toml"
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

/// `oxiremote ui` — spawn the agent detached, wait for /api/health, open
/// browser to `/agent`. If a server is already running on the port, just
/// open the browser.
fn run_ui_command() -> anyhow::Result<()> {
    use std::net::{SocketAddr, TcpStream};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let addr = SocketAddr::from(([127, 0, 0, 1], AGENT_PORT));
    let already_running =
        TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok();

    if !already_running {
        let exe = std::env::current_exe().context("current_exe")?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--tray")
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
        // We deliberately don't wait/reap — child runs independently.
        let pid = child.id();

        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err() {
            anyhow::bail!(
                "background agent did not start within 15s (PID {pid}). Check ~/.oxiremote/."
            );
        }
    }

    let agent_root = format!("http://127.0.0.1:{AGENT_PORT}/agent");
    let _ = open::that(&agent_root);
    println!("OxiRemote running at {agent_root}");
    Ok(())
}

/// Headless path: single tokio runtime on the current (main) thread.
fn run_server_headless() -> anyhow::Result<()> {
    let bus = EventBus::new();
    tracing_setup::init(tracing_setup::AgentMode::Headless, bus.clone());

    let data_dir = default_data_dir()?;
    let _lock = instance_lock::InstanceLock::acquire(&data_dir)
        .context("acquire instance lock")?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async move { server_main(bus).await })
}

/// TUI path: tokio runtime on a sibling thread, TUI event loop on main.
/// The event bus is shared so tunnel/device events drive the TUI live.
fn run_with_tui() -> anyhow::Result<()> {
    let bus = EventBus::new();
    tracing_setup::init(tracing_setup::AgentMode::Tui, bus.clone());

    let data_dir = default_data_dir()?;
    let lock = instance_lock::InstanceLock::acquire(&data_dir)
        .context("acquire instance lock")?;

    let db_path = data_dir.join("oxiremote.sqlite");

    let bus_for_server = bus.clone();
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
            if let Err(err) = rt.block_on(server_main(bus_for_server.clone())) {
                bus_for_server.send(crate::events::AgentEvent::LogEntry {
                    level: crate::events::LogLevel::Error,
                    module: "agent".into(),
                    ts: 0,
                    msg: format!("server exited: {err:#}"),
                });
            }
        })
        .context("spawn server thread")?;

    let result = tui::run_tui(bus, db_path);
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

async fn server_main(event_bus: Arc<EventBus>) -> anyhow::Result<()> {
    let data_dir = default_data_dir()?;
    std::fs::create_dir_all(&data_dir).context("create data dir")?;

    let cloudflared = tunnel::ensure_cloudflared(&data_dir).await.context("ensure cloudflared")?;
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

    let secure_cookies = std::env::var("OXI_SECURE_COOKIES")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

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

    let state = Arc::new(AppState {
        db_path,
        signing_key,
        secure_cookies,
        terminal_sessions: DashMap::new(),
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
    });

    // Background: periodic listening-port discovery + preview health checks.
    local_sites::spawn_discovery_loop(local_sites_cache);
    preview::spawn_health_loop(state.clone());

    // Snapshot maintainer — single bus subscriber that mirrors the latest
    // `TunnelStepChanged` and the last `LOG_RING_CAP` `LogEntry` events into
    // AppState. SSE late-joiners hydrate from `/api/agent/state` and
    // `/api/agent/logs/recent` instead of starting empty.
    {
        let snap_state = state.clone();
        let mut rx = state.event_bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => match &ev {
                        AgentEvent::TunnelStepChanged { .. } => {
                            if let Ok(mut g) = snap_state.latest_tunnel_step.write() {
                                *g = Some(ev.clone());
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

    // Desktop notifications (no-op when no notification daemon — headless,
    // sandboxed, Codespaces). Tray runtime is dead code until Phase 06; this
    // keeps device-pending toasts working in TUI/headless modes today.
    notifier::spawn_event_notifier(state.event_bus.clone());

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
        // Local-sites reverse proxy (Phase 02). Default-deny allowlist is
        // checked inside the handler; bare `/proxy/<port>` redirects to the
        // trailing-slash form so relative paths in upstream HTML resolve.
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
    // Axum's `.layer()` applies in REVERSE order of declaration, so we reverse
    // the list visually here.
    let rate_limiter = state.rate_limiter.clone();
    let app = app
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

    let listener = tokio::net::TcpListener::bind(addr).await?;
    notifier::show_startup(addr);

    let pairing = http_pages::create_pairing_code(&state).context("create pairing code")?;
    info!(pairing_code = %pairing.code, "pair to continue");

    // Start tunnel in background — don't block the HTTP server.
    // If `~/.config/oxiremote/tunnel.toml` exists, run a named tunnel; else
    // fall back to a Quick Tunnel for the dev/first-run experience.
    let named_cfg = tunnel_named::load().unwrap_or(None);
    let tunnel_state = state.clone();
    tokio::spawn(async move {
        let url = match named_cfg {
            Some(cfg) => match tunnel::ensure_named_tunnel(cloudflared, cfg).await {
                Ok(target) => {
                    info!(%target, "named tunnel ready");
                    Some(target)
                }
                Err(err) => {
                    warn!(error=%err, "named tunnel failed");
                    None
                }
            },
            None => match tunnel::ensure_quick_tunnel(addr, cloudflared, tunnel_state.event_bus.clone()).await {
                Ok(url) => {
                    info!(%url, "quick tunnel ready");
                    Some(url)
                }
                Err(err) => {
                    warn!(error=%err, "quick tunnel failed");
                    None
                }
            },
        };
        if let Some(u) = url {
            if let Ok(mut guard) = tunnel_state.tunnel_url.write() {
                *guard = Some(u.clone());
            }
            tunnel_state
                .event_bus
                .send(events::AgentEvent::TunnelUrlChanged { url: u.clone() });

            // Step 4 — tunnel transport up; begin health probes.
            tunnel_state.event_bus.send(events::AgentEvent::TunnelStepChanged {
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
            let probe_bus = tunnel_state.event_bus.clone();
            let probe_url = u.clone();
            let probe_client = tunnel_state.http_client.clone();
            let probe_fut = async move {
                health_check::run_health_check(probe_url, probe_bus, probe_client).await
            };

            let mut client_rx = tunnel_state.event_bus.subscribe();
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

            let healthy = tokio::select! {
                h = probe_fut => h,
                c = first_client_fut => c,
            };

            if healthy {
                // Step 5 — probe passed OR a real client connected.
                tunnel_state.event_bus.send(events::AgentEvent::TunnelStepChanged {
                    step: events::TunnelStep::Ready,
                    attempt: 1,
                    info: Some(u),
                    reason: None,
                });
            } else {
                warn!("tunnel URL did not pass health check within timeout");
                tunnel_state.event_bus.send(events::AgentEvent::TunnelStepChanged {
                    step: events::TunnelStep::Failed,
                    attempt: 0,
                    info: None,
                    reason: Some(
                        "health probe timeout (180s); DNS may not have propagated".into(),
                    ),
                });
            }
        }
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received")
}

async fn api_health() -> &'static str {
    "ok"
}
