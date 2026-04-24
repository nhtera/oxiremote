use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::debug;

use crate::auth::require_active_auth;
use crate::{AppState, AGENT_PORT};

pub type LocalSitesCache = Arc<RwLock<Vec<u16>>>;

pub fn new_cache() -> LocalSitesCache {
    Arc::new(RwLock::new(Vec::new()))
}

pub fn spawn_discovery_loop(cache: LocalSitesCache) {
    tokio::spawn(async move {
        loop {
            let ports = discover_listening_ports().await.unwrap_or_else(|e| {
                debug!(error=%e, "port discovery failed");
                Vec::new()
            });
            {
                let mut write = cache.write().await;
                *write = ports;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

pub async fn discover_listening_ports() -> anyhow::Result<Vec<u16>> {
    #[cfg(target_os = "windows")]
    let ports = netstat_windows().await?;
    #[cfg(not(target_os = "windows"))]
    let ports = lsof_unix().await?;

    let mut filtered: Vec<u16> = ports
        .into_iter()
        .filter(|&p| p != AGENT_PORT && p > 0)
        .collect();
    filtered.sort_unstable();
    filtered.dedup();
    Ok(filtered)
}

#[cfg(not(target_os = "windows"))]
async fn lsof_unix() -> anyhow::Result<Vec<u16>> {
    // `lsof` can stall on hung network FDs — bound it and kill the child if we time out.
    let fut = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "n"])
        .kill_on_drop(true)
        .output();
    let output = match timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => anyhow::bail!("lsof timed out"),
    };
    if !output.status.success() {
        anyhow::bail!("lsof exit: {:?}", output.status);
    }
    Ok(parse_lsof_output(&String::from_utf8_lossy(&output.stdout)))
}

/// `lsof -F n` emits one record per line, name fields prefixed with `n`.
/// Name examples: `n*:8787`, `n127.0.0.1:5173`, `n[::1]:3000 (LISTEN)`.
pub fn parse_lsof_output(text: &str) -> Vec<u16> {
    let mut out = Vec::new();
    for line in text.lines() {
        let rest = match line.strip_prefix('n') {
            Some(r) => r,
            None => continue,
        };
        let endpoint = rest.split_whitespace().next().unwrap_or(rest);
        if let Some(idx) = endpoint.rfind(':') {
            let port_str = &endpoint[idx + 1..];
            if let Ok(port) = port_str.parse::<u16>()
                && port > 0 {
                    out.push(port);
                }
        }
    }
    out
}

#[cfg(target_os = "windows")]
async fn netstat_windows() -> anyhow::Result<Vec<u16>> {
    let fut = Command::new("netstat")
        .args(["-ano"])
        .kill_on_drop(true)
        .output();
    let output = match timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => anyhow::bail!("netstat timed out"),
    };
    if !output.status.success() {
        anyhow::bail!("netstat exit: {:?}", output.status);
    }
    Ok(parse_netstat_output(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse `netstat -ano` output: look for `LISTENING` state, second column
/// is the local address `<host>:<port>`. `[::]:8787` for IPv6.
#[cfg(target_os = "windows")]
pub fn parse_netstat_output(text: &str) -> Vec<u16> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.to_ascii_uppercase().contains("LISTENING") {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let local = cols[1];
        if let Some(idx) = local.rfind(':') {
            if let Ok(port) = local[idx + 1..].parse::<u16>() {
                if port > 0 {
                    out.push(port);
                }
            }
        }
    }
    out
}

pub async fn api_local_sites(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    if require_active_auth(&state.db_path, &state.signing_key, &jar).is_none() {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let read = state.local_sites.read().await;
    Json(read.clone()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_ipv4_and_ipv6() {
        let sample = "p123\ncnode\nPTCP\nn*:5173\nTST=LISTEN\np456\ncpython\nPTCP\nn127.0.0.1:8787\nTST=LISTEN\np789\ncgo\nPTCP\nn[::1]:3000 (LISTEN)\nTST=LISTEN\n";
        let mut ports = parse_lsof_output(sample);
        ports.sort_unstable();
        assert_eq!(ports, vec![3000, 5173, 8787]);
    }

    #[test]
    fn parses_lsof_ignores_non_name_lines() {
        let sample = "p1\ncfoo\nPTCP\nTST=LISTEN\n";
        assert!(parse_lsof_output(sample).is_empty());
    }

    #[test]
    fn parses_lsof_skips_malformed_endpoints() {
        let sample = "nnoport\nn*:notaport\nn:1234\n";
        let ports = parse_lsof_output(sample);
        assert_eq!(ports, vec![1234]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_netstat_listening_lines() {
        let sample = "\
Proto  Local Address          Foreign Address        State           PID\n\
TCP    0.0.0.0:8787           0.0.0.0:0              LISTENING       1234\n\
TCP    [::]:3000              [::]:0                 LISTENING       5678\n\
TCP    10.0.0.1:5173          20.0.0.1:443           ESTABLISHED     999\n";
        let mut ports = parse_netstat_output(sample);
        ports.sort_unstable();
        assert_eq!(ports, vec![3000, 8787]);
    }
}
