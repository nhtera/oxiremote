// Broadcast bus for internal agent events — consumed by TUI, system tray, and
// `/api/agent/events` SSE clients. Overflow drops oldest; lagged subscribers
// re-sync via `/api/agent/state` snapshot.

use std::sync::Arc;

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
    TunnelDown {
        reason: String,
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

#[derive(Debug)]
pub struct EventBus {
    tx: broadcast::Sender<AgentEvent>,
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(EVENT_BUS_CAPACITY);
        Arc::new(Self { tx })
    }

    pub fn send(&self, event: AgentEvent) {
        // No subscribers is fine; drop silently. Overflow (slow receiver) is
        // logged by the consumer via `RecvError::Lagged`.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.tx.subscribe()
    }
}
