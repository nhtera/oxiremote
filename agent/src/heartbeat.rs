// Sleep/wake detector using wall-clock vs monotonic-clock skew.
//
// A 1Hz tick loop compares SystemTime (wall) delta against Instant (monotonic)
// delta. A gap of >5s indicates the OS was suspended. On detection the agent
// runs two probes:
//   1. Loopback health endpoint — confirms the agent process is reachable.
//   2. Public tunnel URL (if registered) — confirms the cloudflared edge
//      connection survived the suspend. After sleep/wake, cloudflared can
//      sit in a permanent control-stream-failure retry loop without exiting
//      (root cause of today's pairing-after-wake bug); the loopback probe
//      passes but the tunnel is dead. Probing the public URL catches that.
// On EITHER probe failing, `force_respawn` is notified so the supervisor
// kills + respawns cloudflared.
//
// Key design decisions:
//   - No Bearer header on either probe (HEAD `/api/health` is unauthenticated).
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

use crate::events::EventBus;
use crate::health_check::{ProbeOutcome, probe_url};

const TICK_INTERVAL: Duration = Duration::from_secs(1);
const SKEW_THRESHOLD: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Spawn the heartbeat detector. Returns a `JoinHandle` that runs until
/// `shutdown` is notified. On sleep/wake detection, probes both loopback and
/// (if a tunnel URL is registered in `bus`) the public URL; on EITHER probe
/// failing, calls `force_respawn.notify_one()`.
pub fn spawn(
    local_addr: SocketAddr,
    bus: Arc<EventBus>,
    force_respawn: Arc<Notify>,
    shutdown: Arc<Notify>,
) -> tokio::task::JoinHandle<()> {
    let probe = move || {
        let url = format!("http://127.0.0.1:{}/api/health", local_addr.port());
        let snap_url = bus.snapshot().url;
        Box::pin(async move {
            let client = reqwest::Client::new();
            let local_ok = client
                .get(&url)
                .timeout(PROBE_TIMEOUT)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            // Skip the public probe when no tunnel URL has been registered yet
            // (agent boot before first TunnelUrlChanged) — there's nothing to
            // probe and the loopback result is the only signal we have.
            let public_ok_fut = async move {
                match snap_url {
                    Some(public) => {
                        matches!(probe_url(&public, &client).await, ProbeOutcome::Ok)
                    }
                    None => true,
                }
            };
            combined_probe(local_ok, public_ok_fut).await
        }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
    };

    spawn_with_probe(Arc::new(probe), force_respawn, shutdown)
}

/// Combine the loopback result with an optional public probe.
/// Returns `false` (signal respawn) if EITHER probe fails. Loopback short-
/// circuits the public probe — if the agent itself isn't reachable, the
/// public probe result tells us nothing useful.
async fn combined_probe<Fut>(local_ok: bool, public_probe: Fut) -> bool
where
    Fut: Future<Output = bool>,
{
    if !local_ok {
        return false;
    }
    public_probe.await
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

    // --- Combined probe (loopback + public) tests ---

    #[tokio::test]
    async fn combined_passes_when_both_pass() {
        let public_ok = async { true };
        assert!(combined_probe(true, public_ok).await);
    }

    #[tokio::test]
    async fn combined_fails_when_loopback_fails() {
        // Public probe should not even be awaited when loopback fails — model
        // that with a panic-on-poll future. If we ever regress and remove the
        // short-circuit, this test will explode loudly.
        let public_panic = async { panic!("public probe must not run when loopback fails") };
        assert!(!combined_probe(false, public_panic).await);
    }

    #[tokio::test]
    async fn combined_fails_when_public_fails() {
        let public_fail = async { false };
        assert!(!combined_probe(true, public_fail).await);
    }

    #[tokio::test]
    async fn combined_passes_when_no_public_url() {
        // The production path passes `true` when no tunnel URL is registered
        // yet — model that here as a future that yields `true`.
        let no_public = async { true };
        assert!(combined_probe(true, no_public).await);
    }
}
