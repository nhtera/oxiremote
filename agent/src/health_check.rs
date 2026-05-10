// HEAD-probe loop against the freshly-issued tunnel URL.
//
// `*.trycloudflare.com` subdomains are anycast on Cloudflare's edge, but the
// system resolver can lag 60-180s before learning the new record (negative
// DNS cache, slow ISP forwarder, VPN caches). Real clients (phones over
// cellular) reach the tunnel via Cloudflare's own resolver and connect
// instantly — only the local probe is stuck. To match that experience we
// resolve the tunnel host via Cloudflare DoH (1.1.1.1) and pin the result
// into a probe-only `reqwest::Client`. DoH unreachable → silent fallback to
// the existing system-DNS client.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::events::{AgentEvent, EventBus};

// Aggressive timeout — the probe is nice-to-have telemetry, not a gate.
// `ensure_quick_tunnel` only returns after cloudflared logs "Registered
// tunnel connection", which IS the real liveness signal. After this window
// expires, main.rs flips to Ready with a soft "(probe inconclusive)"
// suffix rather than blocking the dashboard behind a hard failure.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
const DOH_TIMEOUT: Duration = Duration::from_secs(3);

/// Result of a single HEAD probe against `<tunnel_url>/api/health`.
/// `Err` carries a short human-readable reason ("timeout", "connecting…",
/// HTTP status string, DNS error, …) for log/event surfaces.
#[derive(Debug, Clone)]
pub enum ProbeOutcome {
    Ok,
    Err(String),
}

/// Build a probe-only `reqwest::Client` that resolves `tunnel_url`'s host via
/// Cloudflare DoH and pins the result to IP:443. On any failure (malformed
/// URL, DoH blocked, empty answer) returns the supplied fallback so probes
/// still run via the system resolver. The second tuple element is a
/// human-readable diagnostic string suitable for surfacing to operators.
pub(crate) async fn build_probe_client(
    tunnel_url: &str,
    fallback: &reqwest::Client,
) -> (reqwest::Client, String) {
    let Some(host) = extract_host(tunnel_url) else {
        return (fallback.clone(), "non-https url; system dns".into());
    };
    let doh_started = Instant::now();
    match doh_resolve(&host).await {
        Some(ip) => {
            info!(target: "health_check", host = %host, ip = %ip, "doh resolved");
            let elapsed = doh_started.elapsed().as_millis() as u64;
            let client = build_pinned_client(&host, ip).unwrap_or_else(|| {
                warn!(target: "health_check", "pinned-client build failed; falling back");
                fallback.clone()
            });
            (client, format!("doh resolved → {ip} ({elapsed}ms)"))
        }
        None => {
            warn!(target: "health_check", host = %host, "doh failed; using system dns");
            let elapsed = doh_started.elapsed().as_millis() as u64;
            (
                fallback.clone(),
                format!("doh blocked or failed ({elapsed}ms) → system dns"),
            )
        }
    }
}

/// Run a single HEAD probe against `<tunnel_url>/api/health` using `client`.
/// Caller decides what to do with consecutive failures (one-shot verifying
/// loop in `run_health_check` vs. long-running 3-strike monitor in
/// `edge_health_monitor`). The per-request timeout matches the rest of the
/// probe surface so a slow Cloudflare PoP can't stall the caller's interval.
pub(crate) async fn probe_once(tunnel_url: &str, client: &reqwest::Client) -> ProbeOutcome {
    let health_url = format!("{}/api/health", tunnel_url.trim_end_matches('/'));
    match client
        .head(&health_url)
        .timeout(PER_ATTEMPT_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => ProbeOutcome::Ok,
        Ok(resp) => ProbeOutcome::Err(resp.status().to_string()),
        Err(err) => {
            let reason = if err.is_timeout() {
                "timeout".to_string()
            } else if err.is_connect() {
                "connecting…".to_string()
            } else if let Some(src) = err.source_chain_first() {
                src
            } else {
                err.to_string()
            };
            ProbeOutcome::Err(reason)
        }
    }
}

pub async fn run_health_check(
    tunnel_url: String,
    bus: Arc<EventBus>,
    fallback_client: reqwest::Client,
) -> bool {
    // Build a probe client that resolves the tunnel host via Cloudflare DoH
    // instead of system DNS. On any failure fall back to the system-DNS
    // client so behavior on restricted networks matches today. Surface the
    // outcome via a HealthProbe(attempt=0) diagnostic event.
    let (probe_client, doh_status) = build_probe_client(&tunnel_url, &fallback_client).await;

    bus.send(AgentEvent::HealthProbe {
        attempt: 0,
        status: doh_status,
        elapsed_ms: 0,
        ok: false,
    });

    let start = Instant::now();
    let mut attempt: u32 = 0;

    while start.elapsed() < HEALTH_TIMEOUT {
        attempt += 1;
        let probe_start = Instant::now();
        let outcome = probe_once(&tunnel_url, &probe_client).await;
        let elapsed_ms = probe_start.elapsed().as_millis() as u64;

        match outcome {
            ProbeOutcome::Ok => {
                bus.send(AgentEvent::HealthProbe {
                    attempt,
                    status: "200 OK".into(),
                    elapsed_ms,
                    ok: true,
                });
                return true;
            }
            ProbeOutcome::Err(reason) => {
                bus.send(AgentEvent::HealthProbe {
                    attempt,
                    status: reason,
                    elapsed_ms,
                    ok: false,
                });
            }
        }

        tokio::time::sleep(PROBE_INTERVAL).await;
    }
    false
}

/// Extract the host portion of `https://host[:port][/path]`. Lighter than
/// pulling in the `url` crate for a one-shot helper. Returns `None` for
/// non-HTTPS schemes (the tunnel URL is always HTTPS) or malformed input.
fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url.strip_prefix("https://")?;
    let host_with_port = after_scheme.split('/').next()?;
    let host = host_with_port.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Resolve `host` via Cloudflare's DoH-JSON endpoint. Returns the first A
/// record's IP, or `None` if DoH is unreachable, returns a non-2xx status,
/// returns no Answer section, or returns a malformed IP.
async fn doh_resolve(host: &str) -> Option<IpAddr> {
    let url = format!(
        "https://1.1.1.1/dns-query?name={}&type=A",
        urlencode(host)
    );
    let client = reqwest::Client::builder()
        .timeout(DOH_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(&url)
        .header("Accept", "application/dns-json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // Parse manually via text() — reqwest's `json` feature isn't enabled in
    // this project. serde_json is already a transitive dep.
    let text = resp.text().await.ok()?;
    let body: serde_json::Value = serde_json::from_str(&text).ok()?;
    let answers = body.get("Answer")?.as_array()?;
    for ans in answers {
        let Some(data) = ans.get("data").and_then(|v| v.as_str()) else { continue };
        if let Ok(ip) = data.parse::<IpAddr>() {
            return Some(ip);
        }
    }
    None
}

/// Build a `reqwest::Client` that pins `host` → `ip:443`, bypassing whatever
/// the system resolver thinks. Used only for probe traffic — every probe
/// request targets the same tunnel host, so the pin is comprehensive.
fn build_pinned_client(host: &str, ip: IpAddr) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(PER_ATTEMPT_TIMEOUT + Duration::from_secs(1))
        .resolve(host, SocketAddr::new(ip, 443))
        .build()
        .ok()
}

/// Minimal URL-component encoder for ASCII hostnames. `*.trycloudflare.com`
/// subdomains use only `[a-z0-9-]`, so this is a defense-in-depth strip
/// rather than a real percent-encoder.
fn urlencode(host: &str) -> String {
    host.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        .collect()
}

trait ErrorChain {
    /// First useful description in the source chain — `reqwest::Error`'s
    /// Display is usually a wrapper; the inner cause has the real reason
    /// (DNS, TLS handshake, etc.).
    fn source_chain_first(&self) -> Option<String>;
}

impl ErrorChain for reqwest::Error {
    fn source_chain_first(&self) -> Option<String> {
        let mut src: Option<&dyn std::error::Error> = std::error::Error::source(self);
        while let Some(s) = src {
            let msg = s.to_string();
            if !msg.is_empty() {
                return Some(msg);
            }
            src = std::error::Error::source(s);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(
            extract_host("https://abc.trycloudflare.com").as_deref(),
            Some("abc.trycloudflare.com")
        );
        assert_eq!(
            extract_host("https://abc.trycloudflare.com/api/health").as_deref(),
            Some("abc.trycloudflare.com")
        );
    }

    #[test]
    fn extract_host_strips_port() {
        assert_eq!(
            extract_host("https://abc.trycloudflare.com:443/x").as_deref(),
            Some("abc.trycloudflare.com")
        );
    }

    #[test]
    fn extract_host_rejects_non_https() {
        // The probe always runs against HTTPS tunnels — http/ws aren't valid.
        assert_eq!(extract_host("http://abc.trycloudflare.com"), None);
        assert_eq!(extract_host("ws://abc.trycloudflare.com"), None);
        assert_eq!(extract_host("not-a-url"), None);
        assert_eq!(extract_host(""), None);
    }

    #[test]
    fn extract_host_rejects_empty_host() {
        assert_eq!(extract_host("https:///path"), None);
    }

    #[test]
    fn urlencode_preserves_legal_hostnames() {
        assert_eq!(urlencode("abc.trycloudflare.com"), "abc.trycloudflare.com");
        assert_eq!(urlencode("foo-bar-123.example.com"), "foo-bar-123.example.com");
    }

    #[test]
    fn urlencode_strips_injection_attempts() {
        // Defense-in-depth: even though extract_host returns valid hostnames,
        // confirm that any unexpected characters are stripped before they
        // reach the DoH URL.
        assert_eq!(urlencode("evil.com&injected=1"), "evil.cominjected1");
        assert_eq!(urlencode("a b c"), "abc");
        assert_eq!(urlencode("a/b"), "ab");
    }
}
