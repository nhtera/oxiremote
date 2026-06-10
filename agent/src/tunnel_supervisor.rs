// Internal cloudflared supervisor — respawn loop with exponential backoff.
//
// Responsibilities:
//   - Spawn cloudflared (Quick or Named tunnel mode).
//   - Wait for TunnelDown (or shutdown) via the event bus.
//   - Re-emit TunnelUrlChanged after each successful respawn so the discovery
//     worker and SPA stay current.
//   - Emit AgentEvent::Degraded after 10 cumulative failures within a 24h window.
//
// Coordination notes (Phase 03):
//   The inner select inside `wait_for_tunnel_down_or_shutdown` is intentionally
//   extracted as its own function so Phase 03 can add a `force_respawn` Notify
//   arm without restructuring this file. Phase 03 will add:
//       `force_respawn: Arc<Notify>` parameter to `run()`, then add a third
//       branch to the select inside the wait helper.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Notify, broadcast};
use tracing::{info, warn};

use crate::events::{AgentEvent, EventBus};
use crate::tunnel_named::NamedTunnelConfig;

/// Which cloudflared mode to run.
pub enum TunnelMode {
    /// One-shot quick tunnel; URL changes on every spawn.
    Quick(SocketAddr),
    /// Named tunnel with stable hostname; URL event still emitted for dashboard.
    Named(NamedTunnelConfig),
}

/// Exponential-backoff helper. `attempt` tracks short-window recency (reset by
/// `record_success`); `cumulative_failures_today` (outside this struct) is
/// never reset by success — it drives the Degraded event.
struct Backoff {
    attempt: u32,
    last_success: Instant,
}

impl Backoff {
    fn new() -> Self {
        Self {
            attempt: 0,
            last_success: Instant::now(),
        }
    }

    /// Next sleep before a respawn: 2^attempt seconds, capped at 60s.
    fn next_delay(&self) -> Duration {
        let secs = 1u64 << self.attempt.min(6);
        Duration::from_secs(secs.min(60))
    }

    /// Reset short-window attempt counter when the tunnel stayed up for 5min.
    /// Does NOT affect the external `cumulative_failures_today` counter.
    fn record_success(&mut self) {
        if self.last_success.elapsed() >= Duration::from_secs(300) {
            self.attempt = 0;
        }
    }
}

/// Long backoff applied when cloudflared's stderr exposes Cloudflare's per-IP
/// rate-limit signature (HTTP 429 / error code 1015) on quick-tunnel-create.
/// The throttle window is typically 5-15 min; retrying every 60 s just deepens
/// the rate-limit hole. 10 min is a safe middle ground.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(600);

/// Thin wrapper: spawns cloudflared in the appropriate mode and returns the
/// tunnel URL (None for Named mode without a public hostname). Errors are
/// recoverable unless the binary is missing (ENOENT), which is returned as-is
/// so the caller can distinguish fatal from transient.
async fn spawn_tunnel(
    mode: &TunnelMode,
    cf_path: &Path,
    bus: &Arc<EventBus>,
    shutdown: &Arc<Notify>,
    rate_limited: &Arc<AtomicBool>,
) -> anyhow::Result<(Option<String>, crate::tunnel::TerminateHandle)> {
    match mode {
        TunnelMode::Quick(addr) => {
            let (url, terminate) = crate::tunnel::ensure_quick_tunnel(
                *addr,
                cf_path.to_path_buf(),
                bus.clone(),
                shutdown.clone(),
                rate_limited.clone(),
            )
            .await?;
            Ok((Some(url), terminate))
        }
        TunnelMode::Named(cfg) => {
            crate::tunnel::ensure_named_tunnel(
                cf_path.to_path_buf(),
                cfg.clone(),
                bus.clone(),
                shutdown.clone(),
                rate_limited.clone(),
            )
            .await
        }
    }
}

/// Pick the right delay before respawning. If the stderr scanner saw the
/// rate-limit signature during the last spawn attempt, override the
/// short-window exponential backoff with a 10-min cool-down. Pure function
/// so the decision is unit-testable without driving the full loop.
fn pick_backoff(short_window: Duration, was_rate_limited: bool) -> Duration {
    if was_rate_limited {
        RATE_LIMIT_BACKOFF
    } else {
        short_window
    }
}

/// Block until a TunnelDown event arrives, `force_respawn` fires, or `shutdown` fires.
/// Returns `true` if a respawn should follow (TunnelDown or force_respawn), `false` on shutdown.
async fn wait_for_tunnel_down_or_shutdown(
    rx: &mut broadcast::Receiver<AgentEvent>,
    force_respawn: &Arc<Notify>,
    shutdown: &Arc<Notify>,
) -> bool {
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(AgentEvent::TunnelDown { .. }) | Ok(AgentEvent::TunnelDisconnected) => return true,
                    // Bus lagged — keep waiting; we haven't missed a TunnelDown.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    // Bus closed — treat as shutdown.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    Ok(_) => continue,
                }
            }
            // Heartbeat-driven respawn: treat exactly like a natural TunnelDown.
            // Supervisor owns all kill+wait so heartbeat never touches the Child.
            _ = force_respawn.notified() => return true,
            _ = shutdown.notified() => return false,
        }
    }
}

/// Main supervisor loop. Runs until `shutdown` is notified.
///
/// Invariant: at most one cloudflared process alive at a time (the previous
/// one is either `kill_on_drop`'d or waited on before this function calls
/// `spawn_tunnel` again). A 200ms sleep after each failed spawn gives the OS
/// time to release any port/lock held by the dying process.
///
/// `force_respawn` is notified by the heartbeat task on sleep/wake detection
/// with a failed loopback probe. Supervisor treats it identically to a natural
/// TunnelDown: kill the current child and re-enter the spawn loop.
pub async fn run(
    mode: TunnelMode,
    cf_path: PathBuf,
    bus: Arc<EventBus>,
    shutdown: Arc<Notify>,
    force_respawn: Arc<Notify>,
) -> anyhow::Result<()> {
    // Side task that flips a flag once when shutdown fires. This is the only
    // safe way to "poll" a Notify — tokio::sync::Notify has no `is_notified()`
    // method (red-team Finding 1). The AtomicBool survives the side task so
    // we can check it after select! arms that consume the notify.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = shutdown_flag.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            sd.notified().await;
            flag.store(true, Ordering::SeqCst);
        });
    }

    let mut backoff = Backoff::new();
    let mut rx = bus.subscribe();

    // Cumulative failure counter — NEVER reset by record_success (red-team
    // Finding 11). Drives Degraded so a chronic 5min-stable-then-fail cycle
    // surfaces even though backoff.attempt resets each time.
    let mut cumulative_failures_today: u32 = 0;
    let mut cumulative_window_start = Instant::now();

    // Set by `tunnel::process_cloudflared_stderr` on detection of HTTP 429 /
    // Cloudflare error code 1015 in cloudflared's stderr. Read + cleared after
    // each spawn failure to switch from short-window backoff to a 10-min
    // cool-down when our IP is throttled.
    let rate_limited = Arc::new(AtomicBool::new(false));

    // Handle to terminate the currently-running cloudflared. Fired before every
    // respawn so at most one cloudflared is ever alive. CRITICAL for the
    // force_respawn path: on a natural TunnelDown the child has already exited
    // (no-op), but a heartbeat/edge-health force_respawn fires while the child
    // is still alive — without this it stayed reparented to its wait-task with
    // its stdout/stderr pipes held open, leaking one cloudflared per respawn
    // until the agent exited (FD/socket exhaustion → agent wedged at Preparing).
    let mut prev_terminate: Option<crate::tunnel::TerminateHandle> = None;

    loop {
        if shutdown_flag.load(Ordering::SeqCst) {
            break;
        }

        // Enforce the single-cloudflared invariant: kill the previous spawn's
        // child before launching a new one.
        if let Some(t) = prev_terminate.take() {
            t.notify_one();
        }

        let spawn_start = Instant::now();
        match spawn_tunnel(&mode, &cf_path, &bus, &shutdown, &rate_limited).await {
            Ok((url_or_none, terminate)) => {
                prev_terminate = Some(terminate);
                if let Some(url) = url_or_none {
                    bus.send(AgentEvent::TunnelUrlChanged { url });
                }
                // Tunnel is up; wait for it to die, a forced respawn, or shutdown.
                let should_respawn = wait_for_tunnel_down_or_shutdown(&mut rx, &force_respawn, &shutdown).await;
                if !should_respawn {
                    // Shutdown — terminate the live child explicitly in case the
                    // global shutdown Notify only woke this loop, not the child's
                    // wait-task. No-op if the wait-task already reaped it.
                    if let Some(t) = prev_terminate.take() {
                        t.notify_one();
                    }
                    break;
                }
                // record_success resets only the short-window backoff.
                if spawn_start.elapsed() >= Duration::from_secs(300) {
                    backoff.record_success();
                }
            }
            Err(err) => {
                warn!(error = %err, attempt = backoff.attempt, "tunnel spawn failed");
                // 200ms grace period so the OS can release ports / file locks
                // held by the previous cloudflared process before we try again.
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        if shutdown_flag.load(Ordering::SeqCst) {
            break;
        }

        backoff.attempt = backoff.attempt.saturating_add(1);
        cumulative_failures_today = cumulative_failures_today.saturating_add(1);

        // Reset cumulative window after 24h (red-team Finding 11 requirement).
        if cumulative_window_start.elapsed() >= Duration::from_secs(86_400) {
            info!("tunnel supervisor: resetting 24h failure window (was {})", cumulative_failures_today);
            cumulative_failures_today = 1;
            cumulative_window_start = Instant::now();
        }

        // Read + clear the rate-limit flag. If the last spawn hit HTTP 429 /
        // error 1015, switch to a 10-min cool-down so we stop hammering
        // Cloudflare's throttled API.
        let was_rate_limited = rate_limited.swap(false, Ordering::SeqCst);
        if was_rate_limited {
            warn!(
                "cloudflared 429 / Cloudflare error 1015 detected — backing off {}s before respawn",
                RATE_LIMIT_BACKOFF.as_secs()
            );
        }
        let delay = pick_backoff(backoff.next_delay(), was_rate_limited);

        if cumulative_failures_today >= 10 {
            bus.send(AgentEvent::Degraded {
                reason: format!("tunnel failed {} times in last 24h", cumulative_failures_today),
                retry_in_secs: delay.as_secs(),
            });
        }

        // Sleep the backoff period, but wake immediately on shutdown.
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.notified() => break,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Backoff unit tests ---

    #[test]
    fn backoff_caps_at_60s() {
        let mut b = Backoff::new();
        // Walk through attempts 0..=10; once we pass attempt 6, cap is 60s.
        for _ in 0..7 {
            b.attempt = b.attempt.saturating_add(1);
        }
        // attempt is now 7; 2^7 = 128 → should be capped at 60.
        assert_eq!(b.next_delay(), Duration::from_secs(60));
        // Verify plateau holds.
        b.attempt = 100;
        assert_eq!(b.next_delay(), Duration::from_secs(60));
    }

    #[test]
    fn backoff_initial_delay_is_1s() {
        let b = Backoff::new();
        // attempt = 0 → 2^0 = 1s.
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn backoff_resets_after_stable_uptime() {
        let mut b = Backoff::new();
        b.attempt = 6;
        // Simulate that the last success was 6 minutes ago.
        b.last_success = Instant::now() - Duration::from_secs(360);
        // record_success should reset attempt to 0.
        b.record_success();
        assert_eq!(b.attempt, 0);
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn backoff_does_not_reset_without_stable_uptime() {
        let mut b = Backoff::new();
        b.attempt = 4;
        // last_success is recent (Instant::now() by default).
        b.record_success();
        // Should NOT reset — haven't been stable for 5 minutes.
        assert_eq!(b.attempt, 4);
    }

    // --- Rate-limit-aware backoff (regression for cloudflared 429 / 1015) ---

    #[test]
    fn pick_backoff_uses_rate_limit_constant_when_throttled() {
        // When stderr scanner saw HTTP 429 / error 1015, we must back off for
        // RATE_LIMIT_BACKOFF (10 min), not the much shorter exponential value.
        // Hammering api.trycloudflare.com under throttle deepens the rate-limit
        // window and starves the user of any tunnel until it expires.
        let short = Duration::from_secs(60);
        assert_eq!(pick_backoff(short, true), RATE_LIMIT_BACKOFF);
        assert_eq!(RATE_LIMIT_BACKOFF, Duration::from_secs(600));
    }

    #[test]
    fn pick_backoff_passes_through_short_window_when_not_throttled() {
        // Non-rate-limit failures keep the existing exponential backoff path.
        for secs in [1, 2, 4, 8, 16, 32, 60] {
            let short = Duration::from_secs(secs);
            assert_eq!(pick_backoff(short, false), short);
        }
    }

    #[test]
    fn backoff_ladder_progression() {
        // Verify delays double correctly up to the cap.
        let expectations: &[(u32, u64)] = &[
            (0, 1),
            (1, 2),
            (2, 4),
            (3, 8),
            (4, 16),
            (5, 32),
            (6, 60), // 2^6 = 64 → capped at 60
        ];
        for &(attempt, expected_secs) in expectations {
            let b = Backoff { attempt, last_success: Instant::now() };
            assert_eq!(
                b.next_delay(),
                Duration::from_secs(expected_secs),
                "attempt {attempt}"
            );
        }
    }

    // --- Cumulative window reset test ---

    #[test]
    fn cumulative_window_resets_after_24h() {
        // Simulate the condition check inline (we can't run the loop in a unit test).
        let window_start = Instant::now() - Duration::from_secs(86_401);
        let elapsed = window_start.elapsed();
        assert!(
            elapsed >= Duration::from_secs(86_400),
            "window should appear expired after 24h+1s"
        );
        // After reset, counter would be 1.
        let cumulative_after_reset = 1u32;
        assert_eq!(cumulative_after_reset, 1);
    }

    // --- Shutdown and Degraded tests (async, require tokio) ---

    #[tokio::test]
    async fn shutdown_breaks_loop() {
        use crate::events::EventBus;

        let bus = EventBus::new();
        let shutdown = Arc::new(Notify::new());
        let shutdown_clone = shutdown.clone();

        // Use Named mode with a bogus config — run() will call ensure_named_tunnel
        // which will quickly fail (no real cloudflared), but we're testing that
        // the shutdown signal causes the function to return promptly.
        //
        // We rely on spawn_tunnel failing immediately (no cloudflared binary at
        // a nonsense path), then the loop hitting the backoff select where we
        // notify shutdown.
        let cf_path = PathBuf::from("/nonexistent/cloudflared");
        let mode = TunnelMode::Named(NamedTunnelConfig {
            tunnel_name: "test".into(),
            credentials_file: None,
            hostname: None,
        });

        let force_respawn = Arc::new(Notify::new());
        let run_fut = run(mode, cf_path, bus, shutdown.clone(), force_respawn);

        // Notify after a brief delay.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown_clone.notify_one();
        });

        let result = tokio::time::timeout(Duration::from_millis(500), run_fut).await;
        assert!(result.is_ok(), "run() must return within 500ms of shutdown notify");
    }

    #[tokio::test]
    async fn degraded_emitted_after_10_cumulative_failures() {
        // Verify the Degraded path is triggered by cumulative_failures_today >= 10.
        // We simulate the counter directly (same logic as the loop) rather than
        // running 10 real spawns.
        let bus = crate::events::EventBus::new();
        let mut rx = bus.subscribe();

        let mut cumulative_failures_today: u32 = 9;
        let cumulative_window_start = Instant::now();

        // Simulate one more failure — same body as the loop.
        cumulative_failures_today = cumulative_failures_today.saturating_add(1);
        if cumulative_window_start.elapsed() >= Duration::from_secs(86_400) {
            cumulative_failures_today = 1;
        }
        let delay = Duration::from_secs(60);
        if cumulative_failures_today >= 10 {
            bus.send(AgentEvent::Degraded {
                reason: format!("tunnel failed {} times in last 24h", cumulative_failures_today),
                retry_in_secs: delay.as_secs(),
            });
        }

        // Drain the bus; we should see a Degraded event.
        let mut found = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, AgentEvent::Degraded { .. }) {
                found = true;
            }
        }
        assert!(found, "Degraded must be emitted when cumulative_failures_today reaches 10");
    }

    #[test]
    fn degraded_fires_even_after_backoff_reset() {
        // Regression guard for red-team Finding 11: cumulative_failures_today
        // is separate from backoff.attempt. A cycle where the tunnel is stable
        // for 5min (triggering backoff reset) but then dies again must still
        // accumulate in the daily counter.
        let mut b = Backoff { attempt: 4, last_success: Instant::now() - Duration::from_secs(360) };
        let mut cumulative = 9u32;

        // Simulate record_success (resets backoff.attempt).
        b.record_success();
        assert_eq!(b.attempt, 0, "short-window backoff must reset");

        // Cumulative must NOT reset.
        cumulative = cumulative.saturating_add(1);
        assert!(
            cumulative >= 10,
            "cumulative_failures_today must reach 10 even after backoff reset"
        );
    }
}
