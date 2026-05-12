// Broadcast bus for internal agent events — consumed by TUI, system tray, and
// `/api/agent/events` SSE clients. Overflow drops oldest; lagged subscribers
// re-sync via `/api/agent/state` snapshot.

use std::sync::{Arc, RwLock};

use serde::Serialize;
use tokio::sync::broadcast;

pub const EVENT_BUS_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Done,
    Active,
    Pending,
}

/// Granular tunnel lifecycle step. Emitted as `AgentEvent::TunnelStepChanged`
/// so both the TUI and the WebUI can render a live 5-step checklist.
///
/// Unit-only on purpose — flattens to a plain string on the wire (`"ready"`,
/// `"failed"`, …) so the SPA can pattern-match it directly. When `Failed`,
/// the human-readable cause rides on the parent event's `reason` field.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStep {
    /// Locating / downloading cloudflared binary.
    Preparing,
    /// cloudflared process spawned, waiting for URL.
    Connecting,
    /// URL captured; tunnel transport is up.
    Tunneling,
    /// Running HTTP health probes to confirm reachability.
    Verifying,
    /// Tunnel healthy and serving requests.
    Ready,
    /// Tunnel transport is up but the agent's edge probe failed in a way that
    /// indicates the public URL is not actually serving traffic (DoH NXDOMAIN,
    /// 5xx, transport error). Distinct from `Failed` — cloudflared is still
    /// running, just not reachable from Cloudflare's POV. The human-readable
    /// cause rides on the parent event's `reason` field.
    Degraded,
    /// Tunnel failed — see `reason` on the parent event.
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    StepChange {
        name: String,
        status: StepStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub: Option<String>,
    },
    LogEntry {
        level: LogLevel,
        module: String,
        ts: i64,
        msg: String,
    },
    TunnelUrlChanged {
        url: String,
    },
    DevicePending {
        device_id: String,
        ip: String,
        ua_parsed: String,
        first_seen: i64,
    },
    DeviceConnected {
        device_id: String,
    },
    DeviceDisconnected {
        device_id: String,
    },
    OtkIssued {
        token_prefix: String,
    },
    OtkUsed {
        token_prefix: String,
    },
    OtkExpired {
        token_prefix: String,
    },
    DeviceApproved {
        device_id: String,
    },
    DeviceRejected {
        device_id: String,
    },
    /// Permanent dashboard API key was rotated. Broadcasts `last4` so other
    /// dashboard tabs can refresh their metadata without revealing the plaintext.
    PermanentKeyRotated {
        last4: String,
    },
    /// Per-attempt result from `health_check::run_health_check`. Streams to
    /// TUI and the web UI so the user sees DNS/health probes ticking through
    /// during the "tunnel up but not yet reachable" window.
    HealthProbe {
        attempt: u32,
        status: String,
        elapsed_ms: u64,
        ok: bool,
    },
    /// cloudflared process exited unexpectedly. Distinct from `TunnelUrlChanged`
    /// (which fires on each new URL, including mid-process rotations).
    /// Consumers must handle this to surface a dead-tunnel indicator; no
    /// auto-restart is performed — a fresh spawn would mint a different URL,
    /// which would silently invalidate active QR codes.
    ///
    /// `recovery_hint` is a short, user-actionable string ("Run …", "Check …")
    /// that the TUI / WebUI can render directly so a dead tunnel is not a
    /// terminal state with no next step. Optional so older serialized events
    /// (mid-rolling-deploy) deserialize cleanly on the SPA.
    TunnelDown {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        recovery_hint: Option<String>,
    },
    /// Discovery worker (Phase 02) issued a fresh temp key after a successful
    /// `apiKey -> tunnelUrl` upsert. Carries only the first 4 chars so log
    /// streams and SSE clients never see the full key. The QR pane reads the
    /// canonical value from `AppState::discovery_temp_key`.
    DiscoveryTempKeyIssued {
        key_prefix: String,
    },
    /// Discovery worker exhausted all retry attempts. The QR falls back to
    /// the embedded-tunnel form; the operator can retry by rotating the OTK
    /// (which re-enters the spawn path on next `TunnelUrlChanged`).
    DiscoveryUnavailable,
    /// Operator-initiated tunnel disconnect via `POST /api/agent/tunnel/disconnect`.
    /// The agent process stays alive; only the cloudflared child + outbound
    /// transport are torn down. SPA listeners use this to transition the
    /// dashboard back to the onboarding card without a full reload.
    TunnelDisconnected,
    /// Tunnel supervisor has failed to keep cloudflared alive `cumulative_failures_today`
    /// times in the last 24h. Tray goes red; cleared on next TunnelUrlChanged.
    /// `reason` must not include filesystem paths or env values the operator
    /// hasn't opted into exposing.
    Degraded {
        reason: String,
        retry_in_secs: u64,
    },
    /// Long-running edge-health monitor saw `consecutive_failures` HEAD probes
    /// against the public tunnel URL fail in a row. Emitted right before the
    /// monitor pokes `force_respawn` so the dashboard / tray can surface the
    /// reason for the imminent cloudflared restart. One-shot — the next
    /// successful probe resets the counter silently.
    EdgeUnhealthy {
        url: String,
        consecutive_failures: u32,
    },
    /// An AI coding agent CLI (claude/codex/cursor/opencode) was detected as
    /// the foreground process in a terminal session. Emitted at most once per
    /// state change — repeated polls that see the same agent are no-ops.
    SessionAgentDetected {
        session_id: String,
        agent_name: String,
    },
    /// The previously-detected agent process is no longer the foreground PG
    /// (it exited or was replaced by another process). Carries duration and
    /// exit code so the notifier can build a rich push payload.
    SessionAgentEnded {
        session_id: String,
        agent_name: String,
        duration_ms: u64,
        exit_code: Option<i32>,
    },
    /// Granular tunnel progress — emitted at each lifecycle step so the TUI
    /// and WebUI can render a 5-row checklist with live sub-text.
    TunnelStepChanged {
        step: TunnelStep,
        /// Monotonically increasing attempt counter (1-based); resets on each
        /// fresh tunnel spawn so the UI can distinguish retried attempts.
        attempt: u32,
        /// Optional human-readable detail string (e.g. cloudflared download URL,
        /// probe HTTP status, error snippet).
        #[serde(skip_serializing_if = "Option::is_none")]
        info: Option<String>,
        /// Set when `step` is `Failed` — the root cause string. Sibling field
        /// (rather than payload on the variant) keeps the wire shape flat:
        /// `{ step: "failed", reason: "…" }` instead of nested objects.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Remote-desktop session resolved its transport pipeline. Fired once per
    /// session right after `pipeline_selection::choose()`. The host dashboard
    /// log surface uses this to count JPEG fallbacks vs auto-H.264 sessions
    /// — the phase-01 telemetry soak success metric is computed from these.
    /// `reason` is the stable identifier from `pipeline_selection::Decision`
    /// (e.g. `"auto-h264"`, `"auto-jpeg-no-client"`, `"forced-jpeg"`).
    PipelineChosen {
        device_id: String,
        pipeline: String,
        reason: String,
    },
    /// Phase-02 audio (scaffold). Emitted when the host begins streaming
    /// system audio over the BUNDLE'd PC for `device_id`. No emitter wired
    /// yet — the variant exists so SSE consumers (host dashboard, notifier)
    /// can subscribe ahead of the WASAPI capture path landing.
    AudioStarted {
        device_id: String,
    },
    /// Phase-02 audio (scaffold). Emitted when the audio task tears down.
    /// `reason` is a short stable identifier ("user_toggle_off",
    /// "session_closed", "wasapi_error", …) — keep it free of host-side
    /// detail strings so it is safe to surface in the dashboard.
    AudioStopped {
        device_id: String,
        reason: String,
    },
    /// Phase-03 ABR controller transitioned between zones (Comfort/Probe/
    /// Recovery). `reason` is a short stable identifier ("loss_5pct",
    /// "rtt_500ms", "recovered", "probe_interval") suitable for dashboards
    /// and metric labels — keep it free of float values so cardinality stays
    /// bounded.
    AbrZoneChange {
        device_id: String,
        from: String,
        to: String,
        reason: String,
        target_bitrate_kbps: u32,
    },
    /// macOS host went to the lock screen (`com.apple.screenIsLocked`). Drives
    /// the SPA's locked overlay and the host-dashboard pill. `unix_ms` is the
    /// agent's observation timestamp — useful for "locked Xm ago" copy.
    HostLocked { unix_ms: i64 },
    /// macOS host returned from the lock screen (`com.apple.screenIsUnlocked`).
    /// SPA hides the overlay and asks the active video session for an IDR so
    /// the first frame after unlock is clean.
    HostUnlocked { unix_ms: i64 },
    /// Stay-awake assertion state changed for this agent. Drives the
    /// "Keeping awake" host-dashboard pill so the user can see when the
    /// agent is suppressing auto-lock and screensaver.
    StayAwakeChanged { active: bool },
    /// One-shot per-session warning: the agent started a desktop session
    /// without the OS Accessibility permission, so synthesised input
    /// (keyboard / mouse) will be unreliable. Surfaced inline above the
    /// remote-desktop video as a yellow banner with a deep link to the
    /// System Settings pane.
    AccessibilityMissing {
        platform: &'static str,
    },
}

/// Lightweight tunnel-state snapshot kept in lockstep with broadcast events.
/// Lets late subscribers (e.g. the TUI dashboard, which subscribes only after
/// the menu picks "Terminal UI") catch up on tunnel progress that already
/// fired before they listened.
#[derive(Debug, Clone, Default)]
pub struct TunnelSnapshot {
    pub url: Option<String>,
    pub latest_step: Option<AgentEvent>,
    pub down_reason: Option<String>,
}

#[derive(Debug)]
pub struct EventBus {
    tx: broadcast::Sender<AgentEvent>,
    snapshot: Arc<RwLock<TunnelSnapshot>>,
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        Arc::new(Self {
            tx,
            snapshot: Arc::new(RwLock::new(TunnelSnapshot::default())),
        })
    }

    pub fn send(&self, event: AgentEvent) {
        // Mirror tunnel-shaped events into the snapshot before broadcasting so
        // a subscriber that arrives between `snapshot()` and `subscribe()` can
        // still see the latest state without coordination.
        if let Ok(mut snap) = self.snapshot.write() {
            match &event {
                AgentEvent::TunnelUrlChanged { url } => {
                    snap.url = Some(url.clone());
                    snap.down_reason = None;
                }
                AgentEvent::TunnelStepChanged { .. } => {
                    snap.latest_step = Some(event.clone());
                }
                AgentEvent::TunnelDown { reason, .. } => {
                    snap.down_reason = Some(reason.clone());
                }
                _ => {}
            }
        }
        // No subscribers is fine; drop silently. Overflow (slow receiver) is
        // logged by the consumer via `RecvError::Lagged`.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.tx.subscribe()
    }

    pub fn snapshot(&self) -> TunnelSnapshot {
        self.snapshot
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_mirrors_tunnel_events_for_late_subscribers() {
        let bus = EventBus::new();
        // Late subscribers (e.g. TUI dashboard after the menu) call snapshot()
        // before subscribe() and must see what already fired.
        bus.send(AgentEvent::TunnelUrlChanged {
            url: "https://abc.trycloudflare.com".into(),
        });
        bus.send(AgentEvent::TunnelStepChanged {
            step: TunnelStep::Ready,
            attempt: 1,
            info: Some("serving".into()),
            reason: None,
        });

        let snap = bus.snapshot();
        assert_eq!(snap.url.as_deref(), Some("https://abc.trycloudflare.com"));
        assert!(matches!(
            snap.latest_step,
            Some(AgentEvent::TunnelStepChanged { step: TunnelStep::Ready, .. })
        ));
        assert!(snap.down_reason.is_none());
    }

    #[test]
    fn lock_screen_event_wire_shape() {
        // SPA branches on `type` + `unix_ms`; flatten check guards against a
        // future serde tag rename or accidental envelope wrapper.
        let locked = serde_json::to_value(AgentEvent::HostLocked { unix_ms: 42 }).unwrap();
        assert_eq!(locked["type"], "host_locked");
        assert_eq!(locked["unix_ms"], 42);

        let unlocked = serde_json::to_value(AgentEvent::HostUnlocked { unix_ms: 99 }).unwrap();
        assert_eq!(unlocked["type"], "host_unlocked");
        assert_eq!(unlocked["unix_ms"], 99);

        let stay = serde_json::to_value(AgentEvent::StayAwakeChanged { active: true }).unwrap();
        assert_eq!(stay["type"], "stay_awake_changed");
        assert_eq!(stay["active"], true);

        let acc =
            serde_json::to_value(AgentEvent::AccessibilityMissing { platform: "macos" }).unwrap();
        assert_eq!(acc["type"], "accessibility_missing");
        assert_eq!(acc["platform"], "macos");
    }

    #[test]
    fn snapshot_records_tunnel_down_and_url_clears_it() {
        let bus = EventBus::new();
        bus.send(AgentEvent::TunnelDown {
            reason: "exit code 1".into(),
            recovery_hint: None,
        });
        assert_eq!(bus.snapshot().down_reason.as_deref(), Some("exit code 1"));

        // A fresh tunnel coming back up clears the down marker so re-entries
        // don't see a stale "tunnel down" overlay after recovery.
        bus.send(AgentEvent::TunnelUrlChanged {
            url: "https://new.trycloudflare.com".into(),
        });
        assert!(bus.snapshot().down_reason.is_none());
    }
}
