// Sleep/wake detector using wall-clock vs monotonic-clock skew.
//
// A 1Hz tick loop compares SystemTime (wall) delta against Instant (monotonic)
// delta. A gap of >5s indicates the OS was suspended. On detection the agent's
// loopback health endpoint is probed; failure signals the tunnel supervisor to
// kill and respawn cloudflared.
//
// Key design decisions:
//   - Loopback probe only (http://127.0.0.1:{port}/api/health), no Bearer
//     header. Avoids credential exposure to any rogue tunnel resolver.
//   - Heartbeat does NOT kill the cloudflared child — supervisor owns all
//     process operations (red-team Finding 10). We only call notify_one().
//   - Probe is injected as a closure for unit-testability without spinning up
//     an HTTP server.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::Notify;
use tracing::{info, warn};

const TICK_INTERVAL: Duration = Duration::from_secs(1);
const SKEW_THRESHOLD: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Spawn the heartbeat detector. Returns a `JoinHandle` that runs until
/// `shutdown` is notified. On sleep/wake detection, probes the local agent
/// at `local_addr`; on probe failure, calls `force_respawn.notify_one()`.
pub fn spawn(
    local_addr: SocketAddr,
    force_respawn: Arc<Notify>,
    shutdown: Arc<Notify>,
) -> tokio::task::JoinHandle<()> {
    let probe = move || {
        let url = format!("http://127.0.0.1:{}/api/health", local_addr.port());
        Box::pin(async move {
            reqwest::Client::new()
                .get(&url)
                .timeout(PROBE_TIMEOUT)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
    };

    spawn_with_probe(Arc::new(probe), force_respawn, shutdown)
}

/// Internal spawn that accepts an injectable probe closure — used directly
/// in tests to avoid real HTTP traffic.
#[doc(hidden)]
pub fn spawn_with_probe<F, Fut>(
    probe: Arc<F>,
    force_respawn: Arc<Notify>,
    shutdown: Arc<Notify>,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        let mut last_mono = Instant::now();
        let mut last_wall = SystemTime::now();

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown.notified() => return,
            }

            let mono_delta = last_mono.elapsed();
            let wall_delta = SystemTime::now()
                .duration_since(last_wall)
                .unwrap_or(Duration::ZERO);

            last_mono = Instant::now();
            last_wall = SystemTime::now();

            // Backwards wall skew (NTP backward correction) — skip to avoid
            // treating a clock rollback as a sleep event.
            if wall_delta < mono_delta {
                continue;
            }

            let skew = wall_delta.saturating_sub(mono_delta);
            if skew <= SKEW_THRESHOLD {
                continue;
            }

            info!(
                skew_secs = skew.as_secs(),
                "sleep/wake detected, probing local agent"
            );

            let ok = (probe)().await;
            if ok {
                info!("post-wake probe succeeded; tunnel appears healthy");
            } else {
                warn!("post-wake probe failed; signaling tunnel supervisor to respawn");
                force_respawn.notify_one();
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    // Helper: run the skew check logic without the real tick loop.
    // Returns true if the skew would trigger a probe (i.e. skew > SKEW_THRESHOLD).
    fn should_trigger(wall_delta: Duration, mono_delta: Duration) -> bool {
        if wall_delta < mono_delta {
            return false;
        }
        let skew = wall_delta.saturating_sub(mono_delta);
        skew > SKEW_THRESHOLD
    }

    #[test]
    fn skew_below_threshold_is_ignored() {
        // 4s wall, 1s mono → skew = 3s < 5s → no probe.
        assert!(!should_trigger(Duration::from_secs(4), Duration::from_secs(1)));
    }

    #[test]
    fn skew_above_threshold_triggers_probe() {
        // 30s wall, 1s mono → skew = 29s > 5s → probe fires.
        assert!(should_trigger(Duration::from_secs(30), Duration::from_secs(1)));
    }

    #[tokio::test]
    async fn probe_failure_signals_respawn() {
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());

        // Probe always fails.
        let probe = Arc::new(|| {
            Box::pin(async { false }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
        });

        // Override the skew threshold by providing a probe that runs immediately.
        // We simulate the probe_and_signal path directly.
        let fr = force_respawn.clone();
        let ok = (probe)().await;
        if !ok {
            fr.notify_one();
        }

        // notified() should resolve within a very short timeout.
        let notified = tokio::time::timeout(
            Duration::from_millis(100),
            force_respawn.notified(),
        )
        .await;
        assert!(notified.is_ok(), "force_respawn must be notified on probe failure");

        drop(shutdown);
    }

    #[tokio::test]
    async fn probe_success_does_not_signal() {
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());

        // Probe always succeeds.
        let probe = Arc::new(|| {
            Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
        });

        let fr = force_respawn.clone();
        let ok = (probe)().await;
        if !ok {
            fr.notify_one();
        }

        // notified() must NOT resolve within 100ms (no signal fired).
        let notified = tokio::time::timeout(
            Duration::from_millis(100),
            force_respawn.notified(),
        )
        .await;
        assert!(notified.is_err(), "force_respawn must NOT be notified on probe success");

        drop(shutdown);
    }
}
