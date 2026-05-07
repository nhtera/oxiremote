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
#[derive(Debug, Clone, Copy, Serialize)]
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

/// Severity for `AgentEvent::SettingsHint`. `Error` hints surface as a
/// blocking banner; `Warn`/`Info` are dismissible.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HintSeverity {
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
    /// (which has one-shot URL semantics). Consumers must handle this to surface
    /// a dead-tunnel indicator; no auto-restart is performed — quick-tunnel URLs
    /// rotate on each spawn, which would silently invalidate active QR codes.
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
    /// Operational hint surfaced as a Localhost-scoped dashboard banner.
    /// `code` is a stable identifier the SPA uses for per-hint dismiss
    /// state (local-storage). Examples: `signing-key-perms-failed`,
    /// `root-workspace-present`. Dashboard polls `/api/agent/hints` on
    /// mount + listens to this event for live updates so a hint emitted
    /// before subscription is not lost.
    SettingsHint {
        code: String,
        severity: HintSeverity,
        /// Short, actionable message rendered inside the banner.
        message: String,
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
    /// A new terminal shell was spawned via `terminal_pty::spawn_terminal_session`.
    /// Drives the dashboard audit log and the desktop tray notifier so the
    /// operator sees every shell that opens — even ones spawned after the
    /// dashboard mounted via the `/api/agent/sessions/recent` ring buffer.
    /// `command` is the program path only (argv[0]); arguments are deliberately
    /// excluded so user-supplied flags like `--password=…` never reach this
    /// audit channel (§H11 privacy clause).
    ShellOpened {
        device_id: String,
        session_id: String,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        ts: i64,
    },
    /// PTY for a previously-opened shell exited. `exit_code` is None when the
    /// child reaper failed (rare; the PTY typically reports a code).
    ShellClosed {
        device_id: String,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        ts: i64,
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
