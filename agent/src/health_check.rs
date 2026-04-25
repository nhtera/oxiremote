// HEAD-probe loop against the freshly-issued tunnel URL. Cloudflare Quick
// Tunnels need 30-60s for the new subdomain to propagate; surfacing each
// attempt to the user removes the "is it stuck?" anxiety.
//
// This deliberately uses the system resolver (no 1.1.1.1 bypass yet — YAGNI).
// If users report long waits we'll add hickory-resolver later.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::events::{AgentEvent, EventBus};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_health_check(
    tunnel_url: String,
    bus: Arc<EventBus>,
    client: reqwest::Client,
) -> bool {
    let health_url = format!("{}/api/health", tunnel_url.trim_end_matches('/'));
    let start = Instant::now();
    let mut attempt: u32 = 0;

    while start.elapsed() < HEALTH_TIMEOUT {
        attempt += 1;
        let probe_start = Instant::now();
        let result = client
            .head(&health_url)
            .timeout(PER_ATTEMPT_TIMEOUT)
            .send()
            .await;
        let elapsed_ms = probe_start.elapsed().as_millis() as u64;

        let event = match result {
            Ok(resp) if resp.status().is_success() => {
                bus.send(AgentEvent::HealthProbe {
                    attempt,
                    status: resp.status().to_string(),
                    elapsed_ms,
                    ok: true,
                });
                return true;
            }
            Ok(resp) => AgentEvent::HealthProbe {
                attempt,
                status: resp.status().to_string(),
                elapsed_ms,
                ok: false,
            },
            Err(err) => {
                let status = if err.is_timeout() {
                    "timeout".to_string()
                } else if err.is_connect() {
                    "connecting…".to_string()
                } else if let Some(src) = err.source_chain_first() {
                    src
                } else {
                    err.to_string()
                };
                AgentEvent::HealthProbe {
                    attempt,
                    status,
                    elapsed_ms,
                    ok: false,
                }
            }
        };
        bus.send(event);

        tokio::time::sleep(PROBE_INTERVAL).await;
    }
    false
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
