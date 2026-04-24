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
