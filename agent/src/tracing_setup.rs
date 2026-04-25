// Tracing pipeline. The fmt-to-stderr writer is unsafe in TUI mode because
// stderr corrupts ratatui's alternate buffer. We always emit through a custom
// `Layer` that converts events into `AgentEvent::LogEntry` on the broadcast
// bus; only headless mode also keeps the stderr writer for `journalctl`/Docker.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

use crate::events::{AgentEvent, EventBus, LogLevel};

#[derive(Clone, Copy, Debug)]
pub enum AgentMode {
    Headless,
    Tui,
}

pub struct BusWriterLayer {
    bus: Arc<EventBus>,
}

impl BusWriterLayer {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

impl<S> Layer<S> for BusWriterLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            _ => LogLevel::Info,
        };

        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);

        let mut msg = visitor.message.unwrap_or_default();
        for (k, v) in visitor.fields {
            if !msg.is_empty() {
                msg.push(' ');
            }
            msg.push_str(&k);
            msg.push('=');
            msg.push_str(&v);
        }

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        self.bus.send(AgentEvent::LogEntry {
            level,
            module: metadata.target().to_string(),
            ts,
            msg,
        });
    }
}

#[derive(Default)]
struct LogVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for LogVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(formatted);
        } else {
            self.fields.push((field.name().to_string(), formatted));
        }
    }
}

/// Install the tracing subscriber. Idempotent at the process level — calling
/// twice will no-op the second call (logs a warning on stderr if mode=Headless).
pub fn init(mode: AgentMode, bus: Arc<EventBus>) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let bus_layer = BusWriterLayer::new(bus);

    let registry = tracing_subscriber::registry().with(env_filter).with(bus_layer);

    match mode {
        AgentMode::Headless => {
            let _ = registry
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .try_init();
        }
        AgentMode::Tui => {
            let _ = registry.try_init();
        }
    }
}
