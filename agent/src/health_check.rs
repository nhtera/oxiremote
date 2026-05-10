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

/// Result of a single probe attempt. The variants split out the failure modes
/// that drive different operator-facing dashboard states (Phase 4):
///
/// - `Ok` → tunnel is serving traffic.
/// - `Timeout` → HEAD timed out. Could be slow local DNS / slow PoP. Real
///   clients (cellular phones via Cloudflare's edge resolver) often still pair
///   fine, so this maps to amber-Ready.
/// - `DohNxdomain` → Cloudflare's own DNS doesn't know the hostname.
///   Definitive: the tunnel is not registered. Maps to red-Degraded.
/// - `HttpError(status)` → server answered with non-2xx. 5xx → Degraded;
///   non-401/403 4xx is treated as Ready (the agent answered, we just hit
///   a route mismatch).
/// - `Network(msg)` → connect/TLS/other transport error. Maps to Degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok,
    Timeout,
    DohNxdomain,
    HttpError(u16),
    Network(String),
}

impl ProbeOutcome {
    /// Short status string for the `HealthProbe` event / log output.
    pub fn status_str(&self) -> String {
        match self {
            ProbeOutcome::Ok => "200 OK".into(),
            ProbeOutcome::Timeout => "timeout".into(),
            ProbeOutcome::DohNxdomain => "doh nxdomain".into(),
            ProbeOutcome::HttpError(code) => format!("HTTP {code}"),
            ProbeOutcome::Network(msg) => msg.clone(),
        }
    }
}

/// Outcome of resolving the tunnel host via Cloudflare DoH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DohResult {
    Resolved(IpAddr),
    /// 1.1.1.1 returned `Status:3` (NXDOMAIN). For freshly-issued Quick
    /// Tunnels this is *transient* — `cloudflared` registers with the edge
    /// before Cloudflare's authoritative DNS has propagated the new record,
    /// so a probe fired immediately after registration can land here while
    /// the tunnel is genuinely healthy. The verifying loop now treats this
    /// as retry-class within `HEALTH_TIMEOUT`; only a sustained NXDOMAIN
    /// across the whole window is mapped to `ProbeOutcome::DohNxdomain`.
    Nxdomain,
    /// Network / parse failure. DoH unreachable on this network. Caller falls
    /// back to system DNS so probes still run on restricted networks.
    Unavailable,
}

/// Outcome of building a probe client.
pub(crate) enum ClientOutcome {
    /// Client is pinned to a Cloudflare-resolved IP.
    Pinned(reqwest::Client),
    /// Pinned-client construction failed — the supplied fallback is reused.
    Fallback(reqwest::Client),
    /// DoH said NXDOMAIN; one-shot callers (`probe_url`) short-circuit. The
    /// long-running monitor's 30 s tick gives DNS plenty of time to
    /// propagate, so a sustained NXDOMAIN is meaningful at that cadence.
    /// (`run_health_check` does its own per-attempt DoH retry instead.)
    DefinitivelyBad,
}

/// Build a probe-only `reqwest::Client` that resolves `tunnel_url`'s host via
/// Cloudflare DoH and pins the result to IP:443. Returns `DefinitivelyBad`
/// when DoH says NXDOMAIN so one-shot callers (`probe_url`) skip the HEAD
/// probe — this is correct only at the long-running monitor's tick cadence.
pub(crate) async fn build_probe_client(
    tunnel_url: &str,
    fallback: &reqwest::Client,
) -> ClientOutcome {
    let Some(host) = extract_host(tunnel_url) else {
        return ClientOutcome::Fallback(fallback.clone());
    };
    match doh_resolve(&host).await {
        DohResult::Resolved(ip) => {
            info!(target: "health_check", host = %host, ip = %ip, "doh resolved");
            match build_pinned_client(&host, ip) {
                Some(client) => ClientOutcome::Pinned(client),
                None => {
                    warn!(target: "health_check", "pinned-client build failed; falling back");
                    ClientOutcome::Fallback(fallback.clone())
                }
            }
        }
        DohResult::Nxdomain => {
            warn!(target: "health_check", host = %host, "doh nxdomain — tunnel host not registered");
            ClientOutcome::DefinitivelyBad
        }
        DohResult::Unavailable => {
            warn!(target: "health_check", host = %host, "doh unavailable; using system dns");
            ClientOutcome::Fallback(fallback.clone())
        }
    }
}

/// Run a single HEAD probe against `<tunnel_url>/api/health` using `client`.
/// Returns a tagged outcome the caller maps to dashboard state.
pub(crate) async fn probe_once(tunnel_url: &str, client: &reqwest::Client) -> ProbeOutcome {
    let health_url = format!("{}/api/health", tunnel_url.trim_end_matches('/'));
    match client
        .head(&health_url)
        .timeout(PER_ATTEMPT_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => ProbeOutcome::Ok,
        Ok(resp) => ProbeOutcome::HttpError(resp.status().as_u16()),
        Err(err) => {
            if err.is_timeout() {
                ProbeOutcome::Timeout
            } else {
                let msg = if err.is_connect() {
                    "connecting…".to_string()
                } else if let Some(src) = err.source_chain_first() {
                    src
                } else {
                    err.to_string()
                };
                ProbeOutcome::Network(msg)
            }
        }
    }
}

/// One-shot DoH-resolve + HEAD probe. Convenience for callers (long-running
/// monitor) that don't need to re-use the client across probes.
pub async fn probe_url(tunnel_url: &str, fallback: &reqwest::Client) -> ProbeOutcome {
    match build_probe_client(tunnel_url, fallback).await {
        ClientOutcome::DefinitivelyBad => ProbeOutcome::DohNxdomain,
        ClientOutcome::Pinned(client) | ClientOutcome::Fallback(client) => {
            probe_once(tunnel_url, &client).await
        }
    }
}

/// Multi-attempt verifying loop. Returns the final `ProbeOutcome` the agent
/// should report:
///
/// - First `Ok` short-circuits with `Ok`.
/// - First `HttpError(5xx)` short-circuits with that error (the server
///   answered, just unhealthy — no point retrying within an 8 s window).
/// - `DohNxdomain` / `Timeout` / non-5xx `HttpError` / `Network` keep
///   retrying until `HEALTH_TIMEOUT`; the last seen outcome is returned at
///   end-of-window.
///
/// NXDOMAIN was previously short-circuited as "definitive" but Quick Tunnels
/// register with the Cloudflare edge ~100 ms before authoritative DNS has
/// propagated the new hostname, so a probe fired immediately after
/// registration commonly lands in NXDOMAIN while the tunnel is healthy.
/// Treating it as retry-class gives propagation a chance within the window.
pub async fn run_health_check(
    tunnel_url: String,
    bus: Arc<EventBus>,
    fallback_client: reqwest::Client,
) -> ProbeOutcome {
    run_health_check_with_resolver(tunnel_url, bus, fallback_client, |h| async move {
        doh_resolve(&h).await
    })
    .await
}

/// Test-injectable variant of [`run_health_check`]. The DoH resolver closure
/// lets paused-time tests simulate "NXDOMAIN at t=0, Resolved at t=2s" —
/// the canonical fresh-Quick-Tunnel propagation pattern this loop exists
/// to mask.
#[doc(hidden)]
pub async fn run_health_check_with_resolver<R, Fut>(
    tunnel_url: String,
    bus: Arc<EventBus>,
    fallback_client: reqwest::Client,
    resolver: R,
) -> ProbeOutcome
where
    R: Fn(String) -> Fut,
    Fut: std::future::Future<Output = DohResult>,
{
    let start = Instant::now();
    let mut attempt: u32 = 0;
    // Default to Timeout so a probe loop that never even gets past DoH
    // failure (e.g. immediate cancellation) returns a consistent outcome.
    let mut last = ProbeOutcome::Timeout;
    // Once we get a Resolved DoH answer, pin the client and reuse it across
    // attempts — no point burning DoH calls when the IP isn't going to
    // change for a Quick Tunnel within an 8 s window.
    let mut probe_client: Option<reqwest::Client> = None;

    while start.elapsed() < HEALTH_TIMEOUT {
        attempt += 1;

        // Lazily build the probe client. We retry DoH on each NXDOMAIN /
        // Unavailable iteration so DNS propagation gets a real chance.
        if probe_client.is_none() {
            let host_opt = extract_host(&tunnel_url);
            let doh = match host_opt.as_deref() {
                Some(h) => resolver(h.to_string()).await,
                None => DohResult::Unavailable,
            };
            match doh {
                DohResult::Resolved(ip) => {
                    let host = host_opt.as_deref().unwrap_or("");
                    if let Some(c) = build_pinned_client(host, ip) {
                        info!(target: "health_check", host = %host, ip = %ip, "doh resolved");
                        bus.send(AgentEvent::HealthProbe {
                            attempt: 0,
                            status: format!("doh resolved → {ip}"),
                            elapsed_ms: 0,
                            ok: false,
                        });
                        probe_client = Some(c);
                    } else {
                        warn!(target: "health_check", "pinned-client build failed; falling back");
                        bus.send(AgentEvent::HealthProbe {
                            attempt: 0,
                            status: "doh resolved; pinned client build failed".into(),
                            elapsed_ms: 0,
                            ok: false,
                        });
                        probe_client = Some(fallback_client.clone());
                    }
                }
                DohResult::Nxdomain => {
                    // Transient for fresh Quick Tunnels. Log once, sleep,
                    // and retry until the window closes.
                    warn!(
                        target: "health_check",
                        host = ?host_opt,
                        attempt,
                        "doh nxdomain — retrying within window (DNS propagation)"
                    );
                    bus.send(AgentEvent::HealthProbe {
                        attempt,
                        status: "doh nxdomain (retrying)".into(),
                        elapsed_ms: 0,
                        ok: false,
                    });
                    last = ProbeOutcome::DohNxdomain;
                    tokio::time::sleep(PROBE_INTERVAL).await;
                    continue;
                }
                DohResult::Unavailable => {
                    warn!(target: "health_check", "doh unavailable; using system dns");
                    bus.send(AgentEvent::HealthProbe {
                        attempt: 0,
                        status: "doh unavailable → system dns".into(),
                        elapsed_ms: 0,
                        ok: false,
                    });
                    probe_client = Some(fallback_client.clone());
                }
            }
        }
        let client = probe_client.as_ref().expect("probe_client set above");

        let probe_start = Instant::now();
        let outcome = probe_once(&tunnel_url, client).await;
        let elapsed_ms = probe_start.elapsed().as_millis() as u64;

        bus.send(AgentEvent::HealthProbe {
            attempt,
            status: outcome.status_str(),
            elapsed_ms,
            ok: matches!(outcome, ProbeOutcome::Ok),
        });

        match &outcome {
            ProbeOutcome::Ok => return ProbeOutcome::Ok,
            // 5xx: the server answered but unhealthy. Don't grind retries.
            ProbeOutcome::HttpError(code) if *code >= 500 => return outcome,
            _ => {}
        }
        last = outcome;

        tokio::time::sleep(PROBE_INTERVAL).await;
    }
    last
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

/// Resolve `host` via Cloudflare's DoH-JSON endpoint. Returns:
///
/// - `Resolved(ip)` on a successful answer with at least one A record.
/// - `Nxdomain` when 1.1.1.1 returns `Status: 3` (definitive — host doesn't
///   exist in Cloudflare's DNS).
/// - `Unavailable` for any other failure (network, parse, no Answer section).
async fn doh_resolve(host: &str) -> DohResult {
    let url = format!(
        "https://1.1.1.1/dns-query?name={}&type=A",
        urlencode(host)
    );
    let Ok(client) = reqwest::Client::builder().timeout(DOH_TIMEOUT).build() else {
        return DohResult::Unavailable;
    };
    let Ok(resp) = client
        .get(&url)
        .header("Accept", "application/dns-json")
        .send()
        .await
    else {
        return DohResult::Unavailable;
    };
    if !resp.status().is_success() {
        return DohResult::Unavailable;
    }
    let Ok(text) = resp.text().await else {
        return DohResult::Unavailable;
    };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&text) else {
        return DohResult::Unavailable;
    };
    // Status: 3 = NXDOMAIN per RFC 8484 / DoH-JSON spec.
    if body.get("Status").and_then(|v| v.as_u64()) == Some(3) {
        return DohResult::Nxdomain;
    }
    let Some(answers) = body.get("Answer").and_then(|v| v.as_array()) else {
        return DohResult::Unavailable;
    };
    for ans in answers {
        let Some(data) = ans.get("data").and_then(|v| v.as_str()) else { continue };
        if let Ok(ip) = data.parse::<IpAddr>() {
            return DohResult::Resolved(ip);
        }
    }
    DohResult::Unavailable
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

    // ---- run_health_check NXDOMAIN-retry (regression for stuck-Degraded) ----

    use crate::events::EventBus;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Defensive: a sustained NXDOMAIN across the whole window still maps
    /// to `DohNxdomain` so the dashboard still flips to red eventually.
    /// (We don't paint over genuinely-broken tunnels.) Uses paused time so
    /// the 8 s window elapses synthetically.
    #[tokio::test(start_paused = true)]
    async fn run_health_check_returns_nxdomain_when_never_resolves() {
        let bus = EventBus::new();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = calls.clone();
        let resolver = move |_host: String| {
            calls_c.fetch_add(1, Ordering::SeqCst);
            async { DohResult::Nxdomain }
        };

        let probe = tokio::spawn(run_health_check_with_resolver(
            "https://x.trycloudflare.com".into(),
            bus.clone(),
            reqwest::Client::new(),
            resolver,
        ));

        // Advance well past HEALTH_TIMEOUT (8 s) plus slack so all internal
        // PROBE_INTERVAL sleeps fire.
        tokio::time::advance(HEALTH_TIMEOUT + Duration::from_secs(2)).await;

        let outcome = probe.await.unwrap();
        assert_eq!(outcome, ProbeOutcome::DohNxdomain);
        // Crucial regression check: the loop must have asked the resolver
        // multiple times within the window. The previous `run_health_check`
        // short-circuited on first NXDOMAIN → resolver called once and the
        // dashboard stuck Degraded forever.
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "loop must retry DoH on NXDOMAIN within window (was {})",
            calls.load(Ordering::SeqCst)
        );
    }
}
