// Tracing pipeline. The fmt-to-stderr writer is unsafe in TUI mode because
// stderr corrupts ratatui's alternate buffer. We always emit through a custom
// `Layer` that converts events into `AgentEvent::LogEntry` on the broadcast
// bus; only headless mode also keeps the stderr writer for `journalctl`/Docker.

use std::path::Path;
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
/// twice will no-op the second call.
///
/// Always writes to `{data_dir}/logs/agent.log` (append mode) so the detached
/// `--tray` child has a usable diagnostic surface even though its stdio is
/// null. Headless mode additionally mirrors to stderr for foreground / Docker /
/// journalctl visibility. The returned `WorkerGuard` MUST be held by the caller
/// for the lifetime of the process — dropping it stops the background flush
/// thread and trailing log lines may be lost.
#[must_use = "drop the WorkerGuard at process shutdown to flush the log file"]
pub fn init(
    mode: AgentMode,
    bus: Arc<EventBus>,
    data_dir: &Path,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let bus_layer = BusWriterLayer::new(bus);

    // Set up the rolling file appender. Best-effort: if the logs dir can't be
    // created (read-only home, etc.) we fall back to bus + stderr only.
    let log_dir = data_dir.join("logs");
    let file_guard = match std::fs::create_dir_all(&log_dir) {
        Ok(()) => {
            // Append a SESSION header BEFORE wiring tracing so even crashes
            // during subscriber init are bracketed by a known boot marker.
            // Matches the "=== SESSION ts pid argv ===" format that other
            // remote-access tools use, so cross-product log triage is easy.
            write_session_header(&log_dir.join("agent.log"));

            let file_appender = tracing_appender::rolling::never(&log_dir, "agent.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false);

            let registry = tracing_subscriber::registry()
                .with(env_filter)
                .with(bus_layer)
                .with(file_layer);

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
            Some(guard)
        }
        Err(_) => {
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
            None
        }
    };

    file_guard
}

/// Write a one-line SESSION marker to the log file. Best-effort: I/O errors
/// are swallowed so a missing/locked log file never blocks startup.
fn write_session_header(path: &Path) {
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let pid = std::process::id();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let argv_str = if argv.is_empty() { String::new() } else { argv.join(" ") };
    let _ = writeln!(
        file,
        "\n=== SESSION {ts} pid={pid} argv={argv_str} version={version} ===",
        version = env!("CARGO_PKG_VERSION")
    );
}
