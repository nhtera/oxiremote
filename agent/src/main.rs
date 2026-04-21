mod auth;
mod db;
mod files;
mod git;
mod http_pages;
mod preview;
mod terminal_api;
mod terminal_pty;
mod terminal_ws;
mod tunnel;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use crate::terminal_pty::TerminalSession;
use crate::preview::PreviewTarget;

const AGENT_PORT: u16 = 8787;

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    pub signing_key: Vec<u8>,
    pub secure_cookies: bool,
    pub terminal_sessions: DashMap<String, Arc<TerminalSession>>,
    pub preview_targets: DashMap<String, PreviewTarget>,
    pub workspace_root: PathBuf,
}

fn default_data_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".oxiremote"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = default_data_dir()?;
    std::fs::create_dir_all(&data_dir).context("create data dir")?;

    let cloudflared = tunnel::ensure_cloudflared(&data_dir).await.context("ensure cloudflared")?;
    info!(path = %cloudflared.display(), "cloudflared ready");

    let db_path = data_dir.join("oxiremote.sqlite");
    db::init_db(&db_path).context("init db")?;

    let key_path = data_dir.join("signing.key");
    let signing_key = auth::load_or_create_key(&key_path).context("load signing key")?;

    let secure_cookies = std::env::var("OXI_SECURE_COOKIES")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let workspace_root = std::env::var("OXI_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let state = Arc::new(AppState {
        db_path,
        signing_key,
        secure_cookies,
        terminal_sessions: DashMap::new(),
        preview_targets: DashMap::new(),
        workspace_root,
    });

    let app = Router::new()
        .route("/", get(http_pages::root))
        .route("/api/health", get(api_health))
        .route("/login", get(http_pages::login_page))
        .route("/app", get(http_pages::app_root))
        .route("/app/", get(http_pages::app_root))
        .route("/api/me", get(http_pages::api_me))
        .route("/api/pairing/start", post(http_pages::api_pairing_start))
        .route("/api/pairing/exchange", post(http_pages::api_pairing_exchange))
        .route("/api/auth/logout", post(http_pages::api_logout))
        // terminal
        .route(
            "/api/terminal/sessions",
            get(terminal_api::api_terminal_sessions_list).post(terminal_api::api_terminal_sessions_create),
        )
        .route("/api/terminal/sessions/{id}/ws", get(terminal_ws::api_terminal_session_ws))
        .route("/api/terminal/sessions/{id}/resize", post(terminal_api::api_terminal_session_resize))
        .route("/api/terminal/sessions/{id}/close", post(terminal_api::api_terminal_session_close))
        // git
        .route("/api/git/status", get(git::api_git_status))
        .route("/api/git/diff", get(git::api_git_diff))
        .route("/api/git/stage", post(git::api_git_stage))
        .route("/api/git/unstage", post(git::api_git_unstage))
        .route("/api/git/commit", post(git::api_git_commit))
        // files
        .route("/api/files/list", get(files::api_files_list))
        .route("/api/files/read", get(files::api_files_read))
        .route("/api/files/write", post(files::api_files_write))
        // preview proxy
        .route("/api/previews", get(preview::api_previews_list).post(preview::api_previews_create))
        .route("/api/previews/{id}", axum::routing::delete(preview::api_previews_delete))
        .route("/preview/{id}/{*rest}", axum::routing::any(preview::preview_proxy_handler))
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], AGENT_PORT));
    info!(%addr, "starting agent server");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let pairing = http_pages::create_pairing_code(&state).context("create pairing code")?;
    info!(pairing_code = %pairing.code, "pair to continue");

    let tunnel_task = tokio::spawn(async move { tunnel::ensure_quick_tunnel(addr, cloudflared).await });

    match tunnel_task.await {
        Ok(Ok(url)) => info!(%url, "quick tunnel ready"),
        Ok(Err(err)) => warn!(error=%err, "quick tunnel failed"),
        Err(err) => warn!(error=%err, "quick tunnel task failed"),
    }

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
