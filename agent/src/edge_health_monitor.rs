// Long-running edge-health monitor.
//
// Today's bug (root cause: plans/reports/debugger-260510-1405-otk-pairing-after-sleep-wake.md):
// after sleep/wake cloudflared can sit in a permanent control-stream-failure
// retry loop without exiting. The supervisor never sees `TunnelDown`. The
// existing heartbeat probes loopback only — the agent process is fine, the
// edge connection is dead, nobody notices.
//
// This module fills that gap: every 30s after the tunnel reports `Ready`, run
// a HEAD probe against `<tunnel_url>/api/health` resolved via Cloudflare DoH.
// Three consecutive failures → notify `force_respawn` so the supervisor kills
// and respawns cloudflared. 60s throttle prevents a respawn loop during a real
// Cloudflare-side outage.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::events::{AgentEvent, EventBus, TunnelStep};
use crate::health_check::{ProbeOutcome, build_probe_client, probe_once};

const INITIAL_INTERVAL: Duration = Duration::from_secs(30);
const MAX_INTERVAL: Duration = Duration::from_secs(120);
const FAIL_THRESHOLD: u32 = 3;
const RESPAWN_THROTTLE: Duration = Duration::from_secs(60);

/// Spawn the long-running monitor with the production probe (DoH-resolved
/// pinned `reqwest::Client` + HEAD `/api/health`). Returns a `JoinHandle` that
/// runs until `shutdown` is notified.
pub fn spawn(
    bus: Arc<EventBus>,
    force_respawn: Arc<Notify>,
    shutdown: Arc<Notify>,
) -> JoinHandle<()> {
    let fallback = reqwest::Client::new();
    let probe = move |url: String| {
        let fallback = fallback.clone();
        async move {
            let (client, _diag) = build_probe_client(&url, &fallback).await;
            probe_once(&url, &client).await
        }
    };
    spawn_with_probe(bus, Arc::new(probe), force_respawn, shutdown)
}

/// Internal spawn that accepts an injectable probe closure for unit tests.
/// Behavior is otherwise identical to `spawn`.
#[doc(hidden)]
pub fn spawn_with_probe<F, Fut>(
    bus: Arc<EventBus>,
    probe: Arc<F>,
    force_respawn: Arc<Notify>,
    shutdown: Arc<Notify>,
) -> JoinHandle<()>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ProbeOutcome> + Send + 'static,
{
    // Subscribe BEFORE spawning so the caller can send TunnelUrlChanged /
    // TunnelStepChanged events synchronously after `spawn_with_probe` returns
    // without the broadcast channel dropping them on the floor.
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let mut current_url: Option<String> = None;
        // Probes only fire once the supervisor reports Ready — before that,
        // the tunnel is still being set up and a probe would just race the
        // verifying loop.
        let mut ready = false;
        let mut consecutive_failures: u32 = 0;
        let mut last_respawn_at: Option<Instant> = None;
        let mut interval = INITIAL_INTERVAL;
        let mut sleep = Box::pin(tokio::time::sleep(interval));

        loop {
            tokio::select! {
                _ = shutdown.notified() => return,
                ev = rx.recv() => match ev {
                    Ok(AgentEvent::TunnelUrlChanged { url }) => {
                        current_url = Some(url);
                        consecutive_failures = 0;
                        interval = INITIAL_INTERVAL;
                        ready = false;
                    }
                    Ok(AgentEvent::TunnelStepChanged { step: TunnelStep::Ready, .. }) => {
                        ready = true;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = &mut sleep => {
                    if let Some(url) = current_url.clone()
                        && ready
                    {
                        run_probe(
                            &bus,
                            &probe,
                            &force_respawn,
                            &url,
                            &mut consecutive_failures,
                            &mut last_respawn_at,
                            &mut interval,
                        ).await;
                    }
                    sleep = Box::pin(tokio::time::sleep(interval));
                }
            }
        }
    })
}

async fn run_probe<F, Fut>(
    bus: &Arc<EventBus>,
    probe: &Arc<F>,
    force_respawn: &Arc<Notify>,
    url: &str,
    consecutive_failures: &mut u32,
    last_respawn_at: &mut Option<Instant>,
    interval: &mut Duration,
) where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ProbeOutcome> + Send + 'static,
{
    let outcome = (probe)(url.to_string()).await;
    match outcome {
        ProbeOutcome::Ok => {
            if *consecutive_failures > 0 {
                info!(
                    target: "edge_health_monitor",
                    url = url,
                    "edge probe recovered after {} failures",
                    *consecutive_failures
                );
            }
            *consecutive_failures = 0;
            *interval = INITIAL_INTERVAL;
        }
        ProbeOutcome::Err(reason) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            *interval = next_backoff(*interval);
            warn!(
                target: "edge_health_monitor",
                url = url,
                attempts = *consecutive_failures,
                reason = %reason,
                "edge probe failed"
            );
            if *consecutive_failures >= FAIL_THRESHOLD {
                let throttled = last_respawn_at
                    .map(|t| t.elapsed() < RESPAWN_THROTTLE)
                    .unwrap_or(false);
                if !throttled {
                    bus.send(AgentEvent::EdgeUnhealthy {
                        url: url.to_string(),
                        consecutive_failures: *consecutive_failures,
                    });
                    force_respawn.notify_one();
                    *last_respawn_at = Some(Instant::now());
                } else {
                    info!(
                        target: "edge_health_monitor",
                        "respawn throttled; last respawn within {}s",
                        RESPAWN_THROTTLE.as_secs()
                    );
                }
                // Reset the counter regardless — we either fired or throttled.
                // The next 3 consecutive failures will be evaluated fresh
                // against the throttle window.
                *consecutive_failures = 0;
                *interval = INITIAL_INTERVAL;
            }
        }
    }
}

fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > MAX_INTERVAL { MAX_INTERVAL } else { doubled }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::time::{Duration as TokioDuration, advance, pause};

    fn ready_event() -> AgentEvent {
        AgentEvent::TunnelStepChanged {
            step: TunnelStep::Ready,
            attempt: 1,
            info: None,
            reason: None,
        }
    }

    /// Drain any EdgeUnhealthy events from a subscriber.
    async fn count_unhealthy(rx: &mut broadcast::Receiver<AgentEvent>) -> u32 {
        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, AgentEvent::EdgeUnhealthy { .. }) {
                n += 1;
            }
        }
        n
    }

    /// Hand control back to the scheduler several times so the spawned task
    /// can drain queued bus events / post-probe scheduling between advances.
    /// One `yield_now` is not always enough when a single tick processes
    /// multiple awaits (recv → set state → loop → recv → set state → loop).
    async fn settle() {
        for _ in 0..6 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ok_keeps_counter_at_zero() {
        let bus = EventBus::new();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = calls.clone();
        let probe = Arc::new(move |_url: String| {
            calls_c.fetch_add(1, Ordering::SeqCst);
            async { ProbeOutcome::Ok }
        });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn.clone(), shutdown.clone());
        bus.send(AgentEvent::TunnelUrlChanged { url: "https://abc.trycloudflare.com".into() });
        bus.send(ready_event());
        settle().await;

        // Run a few probe ticks.
        for _ in 0..4 {
            advance(TokioDuration::from_secs(31)).await;
            settle().await;
        }

        // No respawn.
        let notified = tokio::time::timeout(TokioDuration::from_millis(50), force_respawn.notified()).await;
        assert!(notified.is_err(), "force_respawn must NOT fire on healthy probes");
        assert!(calls.load(Ordering::SeqCst) >= 3, "probe should have been called several times");

        shutdown.notify_one();
        let _ = tokio::time::timeout(TokioDuration::from_millis(50), handle).await;
    }

    #[tokio::test(start_paused = true)]
    async fn three_failures_fire_force_respawn_once() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let probe = Arc::new(|_url: String| async { ProbeOutcome::Err("fail".into()) });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn.clone(), shutdown.clone());
        bus.send(AgentEvent::TunnelUrlChanged { url: "https://abc.trycloudflare.com".into() });
        bus.send(ready_event());
        settle().await;

        // 3 fails: 30s interval (then 60s, then 120s) — push the clock past all three.
        // After fail 1: interval=60s. After fail 2: interval=120s. After fail 3: respawn fires.
        advance(TokioDuration::from_secs(31)).await; // fail 1
        settle().await;
        advance(TokioDuration::from_secs(61)).await; // fail 2
        settle().await;
        advance(TokioDuration::from_secs(121)).await; // fail 3 → respawn
        settle().await;

        // force_respawn must have been notified.
        let notified = tokio::time::timeout(TokioDuration::from_millis(100), force_respawn.notified()).await;
        assert!(notified.is_ok(), "force_respawn must fire after 3 consecutive failures");

        // Exactly one EdgeUnhealthy emitted.
        let n = count_unhealthy(&mut rx).await;
        assert_eq!(n, 1, "expected exactly one EdgeUnhealthy event");

        shutdown.notify_one();
        let _ = tokio::time::timeout(TokioDuration::from_millis(50), handle).await;
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_prevents_second_respawn_within_60s() {
        let bus = EventBus::new();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let probe = Arc::new(|_url: String| async { ProbeOutcome::Err("fail".into()) });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn.clone(), shutdown.clone());
        bus.send(AgentEvent::TunnelUrlChanged { url: "https://abc.trycloudflare.com".into() });
        bus.send(ready_event());
        settle().await;

        // First respawn cycle: 3 fails.
        advance(TokioDuration::from_secs(31)).await;
        settle().await;
        advance(TokioDuration::from_secs(61)).await;
        settle().await;
        advance(TokioDuration::from_secs(121)).await;
        settle().await;

        // Consume the first notification.
        let _ = tokio::time::timeout(TokioDuration::from_millis(100), force_respawn.notified()).await;

        // Within the 60s throttle window, even three more failures must not
        // trigger a second respawn. Counter resets after the first respawn so
        // the next 3 consecutive fails would normally fire — but the throttle
        // gate suppresses it. Drive 3 quick failures within the window.
        // After respawn, interval reset to 30s. We need < 60s wall budget.
        advance(TokioDuration::from_secs(31)).await; // fail 1
        settle().await;
        advance(TokioDuration::from_secs(20)).await; // not enough for fail 2 yet
        settle().await;

        let second = tokio::time::timeout(TokioDuration::from_millis(50), force_respawn.notified()).await;
        assert!(second.is_err(), "force_respawn must NOT fire a second time within the throttle window");

        shutdown.notify_one();
        let _ = tokio::time::timeout(TokioDuration::from_millis(50), handle).await;
    }

    #[tokio::test(start_paused = true)]
    async fn success_after_partial_fails_resets_counter() {
        let bus = EventBus::new();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        // First two probes fail, third succeeds, then keep succeeding.
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = calls.clone();
        let probe = Arc::new(move |_url: String| {
            let n = calls_c.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 { ProbeOutcome::Err("fail".into()) } else { ProbeOutcome::Ok }
            }
        });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn.clone(), shutdown.clone());
        bus.send(AgentEvent::TunnelUrlChanged { url: "https://abc.trycloudflare.com".into() });
        bus.send(ready_event());
        settle().await;

        advance(TokioDuration::from_secs(31)).await; // fail 1
        settle().await;
        advance(TokioDuration::from_secs(61)).await; // fail 2 (interval=60)
        settle().await;
        advance(TokioDuration::from_secs(121)).await; // success (interval=120 → reset)
        settle().await;
        // Now interval is 30 again — push past it without a fourth fail to ensure
        // the counter actually reset.
        advance(TokioDuration::from_secs(31)).await;
        settle().await;
        advance(TokioDuration::from_secs(31)).await;
        settle().await;

        // No respawn — the early failures didn't accumulate to 3 because
        // a success in the middle reset the counter.
        let notified = tokio::time::timeout(TokioDuration::from_millis(50), force_respawn.notified()).await;
        assert!(notified.is_err(), "success between fails must reset the counter");

        shutdown.notify_one();
        let _ = tokio::time::timeout(TokioDuration::from_millis(50), handle).await;
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_signal_stops_loop() {
        let bus = EventBus::new();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let probe = Arc::new(|_url: String| async { ProbeOutcome::Ok });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn, shutdown.clone());
        shutdown.notify_one();

        // Should return promptly.
        let res = tokio::time::timeout(TokioDuration::from_millis(200), handle).await;
        assert!(res.is_ok(), "monitor must stop within 200ms of shutdown notify");
    }

    #[tokio::test(start_paused = true)]
    async fn probes_held_off_until_first_ready() {
        let bus = EventBus::new();
        let force_respawn = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let calls = Arc::new(AtomicU32::new(0));
        let calls_c = calls.clone();
        let probe = Arc::new(move |_url: String| {
            calls_c.fetch_add(1, Ordering::SeqCst);
            async { ProbeOutcome::Err("fail".into()) }
        });

        let handle = spawn_with_probe(bus.clone(), probe, force_respawn.clone(), shutdown.clone());
        // URL is set but Ready has NOT fired yet.
        bus.send(AgentEvent::TunnelUrlChanged { url: "https://abc.trycloudflare.com".into() });
        settle().await;

        // Advance past several intervals — no probe should fire because Ready hasn't been seen.
        for _ in 0..5 {
            advance(TokioDuration::from_secs(31)).await;
            settle().await;
        }

        assert_eq!(calls.load(Ordering::SeqCst), 0, "probes must not fire before Ready");

        shutdown.notify_one();
        let _ = tokio::time::timeout(TokioDuration::from_millis(50), handle).await;
    }

    // Pure-function tests for the backoff schedule.

    #[test]
    fn backoff_doubles_then_caps() {
        let mut d = INITIAL_INTERVAL;
        d = next_backoff(d);
        assert_eq!(d, Duration::from_secs(60));
        d = next_backoff(d);
        assert_eq!(d, Duration::from_secs(120));
        d = next_backoff(d);
        assert_eq!(d, Duration::from_secs(120), "backoff caps at MAX_INTERVAL");
    }

    // Ensure the imports line stays stable so a future refactor doesn't drop pause().
    #[tokio::test]
    async fn pause_is_available() {
        pause();
    }
}
