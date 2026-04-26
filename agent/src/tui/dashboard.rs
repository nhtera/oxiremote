// Main TUI dashboard. Shows the pairing QR (encoding `<tunnel>/login?k=<otk>`
// so a phone scan auto-fills the OTK), live OTK + countdown, host info, and a
// hotkey panel. Subscribes to the event bus so tunnel/device/OTK state updates
// live-refresh without polling.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use qrcode::{QrCode, render::unicode};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::approval;
use crate::events::{AgentEvent, EventBus, StepStatus};
use crate::{approval as approval_db, one_time_keys};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

struct State {
    db_path: PathBuf,
    tunnel_url: Option<String>,
    steps: Vec<super::step_progress::Step>,
    connected_devices: usize,
    last_log: Option<String>,
    /// Last few HEAD-probe attempts; rendered as a tiny streaming console
    /// while the tunnel is up but health-check hasn't passed yet.
    probe_log: Vec<ProbeEntry>,
    otk_token: Option<String>,
    otk_expires_at: Option<i64>,
    /// Toggles whether the body shows the QR + info panel or the recent log
    /// stream. `l` keypress flips it.
    show_logs: bool,
    log_history: Vec<String>,
    /// Transient one-line status message — rendered in the footer for ~3 s
    /// after a hotkey action like "Copied URL" or "OTK regenerated".
    flash: Option<(String, SystemTime)>,
}

#[derive(Clone)]
struct ProbeEntry {
    attempt: u32,
    status: String,
    ok: bool,
    elapsed_ms: u64,
}

const PROBE_LOG_MAX: usize = 6;
const LOG_HISTORY_MAX: usize = 50;
const FLASH_TTL: Duration = Duration::from_secs(3);

impl State {
    fn new(db_path: PathBuf) -> Self {
        use super::step_progress::Step;
        let initial_otk = one_time_keys::active_otk(&db_path).ok().flatten();
        let (otk_token, otk_expires_at) = match initial_otk {
            Some(rec) => (Some(rec.token), Some(rec.expires_at)),
            None => (None, None),
        };
        Self {
            db_path,
            tunnel_url: None,
            steps: vec![
                Step {
                    name: "Server".into(),
                    status: StepStatus::Done,
                    sub: Some("localhost:8787".into()),
                },
                Step {
                    name: "Tunnel".into(),
                    status: StepStatus::Active,
                    sub: Some("starting…".into()),
                },
                Step {
                    name: "Ready".into(),
                    status: StepStatus::Pending,
                    sub: None,
                },
            ],
            connected_devices: 0,
            last_log: None,
            probe_log: Vec::new(),
            otk_token,
            otk_expires_at,
            show_logs: false,
            log_history: Vec::new(),
            flash: None,
        }
    }

    fn ready_verifying(&self) -> bool {
        self.steps
            .iter()
            .find(|s| s.name == "Ready")
            .map(|s| matches!(s.status, StepStatus::Active))
            .unwrap_or(false)
    }

    fn refresh_otk_from_db(&mut self) {
        match one_time_keys::active_otk(&self.db_path) {
            Ok(Some(rec)) => {
                self.otk_token = Some(rec.token);
                self.otk_expires_at = Some(rec.expires_at);
            }
            _ => {
                self.otk_token = None;
                self.otk_expires_at = None;
            }
        }
    }

    fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), SystemTime::now()));
    }

    fn current_flash(&self) -> Option<&str> {
        self.flash.as_ref().and_then(|(msg, ts)| {
            let elapsed = SystemTime::now().duration_since(*ts).unwrap_or_default();
            if elapsed < FLASH_TTL { Some(msg.as_str()) } else { None }
        })
    }

    fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TunnelUrlChanged { url } => {
                self.tunnel_url = Some(url.clone());
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Tunnel") {
                    s.status = StepStatus::Done;
                    s.sub = Some(url.clone());
                }
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Ready") {
                    s.status = StepStatus::Active;
                    s.sub = Some("verifying…".into());
                }
            }
            AgentEvent::HealthProbe { attempt, status, ok, elapsed_ms } => {
                self.probe_log.push(ProbeEntry {
                    attempt: *attempt,
                    status: status.clone(),
                    ok: *ok,
                    elapsed_ms: *elapsed_ms,
                });
                while self.probe_log.len() > PROBE_LOG_MAX {
                    self.probe_log.remove(0);
                }
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Ready") {
                    if *ok {
                        s.status = StepStatus::Done;
                        s.sub = Some("waiting for devices".into());
                    } else {
                        s.sub = Some(format!("#{attempt} → {status}"));
                    }
                }
            }
            AgentEvent::DeviceConnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_add(1);
            }
            AgentEvent::DeviceDisconnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_sub(1);
            }
            AgentEvent::LogEntry { msg, .. } => {
                self.last_log = Some(msg.clone());
                self.log_history.push(msg.clone());
                if self.log_history.len() > LOG_HISTORY_MAX {
                    self.log_history.remove(0);
                }
            }
            AgentEvent::OtkIssued { .. } | AgentEvent::OtkUsed { .. } | AgentEvent::OtkExpired { .. } => {
                self.refresh_otk_from_db();
            }
            _ => {}
        }
    }

    /// Combine tunnel URL + active OTK into the deep-link the QR encodes.
    /// Mirrors `pairing-card.tsx` so a phone scan reaches /login with `k=`
    /// pre-filled and submits without keyboard typing.
    fn qr_payload(&self) -> String {
        match (&self.tunnel_url, &self.otk_token) {
            (Some(url), Some(otk)) => {
                format!("{}/login?k={}", url.trim_end_matches('/'), otk)
            }
            (Some(url), None) => url.clone(),
            _ => "Tunnel not ready yet…".to_string(),
        }
    }
}

pub fn run_dashboard(term: &mut Term, event_bus: Arc<EventBus>, db_path: PathBuf) -> Result<()> {
    let mut rx = event_bus.subscribe();
    let mut state = State::new(db_path);

    loop {
        term.draw(|f| draw(f, &state))?;

        // Drain buffered events without blocking so redraws stay responsive.
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::DevicePending { .. } = &event {
                approval::run_approval(term, &event)?;
                continue;
            }
            state.apply(&event);
        }

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('o') => {
                    if let Some(url) = &state.tunnel_url {
                        let _ = open::that(url.as_str());
                    }
                }
                KeyCode::Char('h') => {
                    let _ = open::that("http://localhost:8787/agent");
                }
                KeyCode::Char('c') => {
                    if let Some(url) = state.tunnel_url.clone() {
                        match copy_to_clipboard(&url) {
                            Ok(()) => state.set_flash("Copied tunnel URL"),
                            Err(_) => state.set_flash("Clipboard unavailable"),
                        }
                    } else {
                        state.set_flash("Tunnel not ready yet");
                    }
                }
                KeyCode::Char('k') => match state.otk_token.clone() {
                    Some(token) => match copy_to_clipboard(&token) {
                        Ok(()) => state.set_flash("Copied one-time key"),
                        Err(_) => state.set_flash("Clipboard unavailable"),
                    },
                    None => state.set_flash("No active key — press r to generate"),
                },
                KeyCode::Char('r') => match one_time_keys::generate_otk(&state.db_path, None) {
                    Ok(rec) => {
                        let prefix: String = rec.token.chars().take(4).collect();
                        event_bus.send(AgentEvent::OtkIssued { token_prefix: prefix });
                        state.otk_token = Some(rec.token);
                        state.otk_expires_at = Some(rec.expires_at);
                        state.set_flash("Regenerated one-time key");
                    }
                    Err(err) => state.set_flash(format!("OTK error: {err}")),
                },
                KeyCode::Char('a') => match approval_db::list_pending(&state.db_path) {
                    Ok(devices) => match devices.first() {
                        Some(dev) => match approval_db::approve_device(&state.db_path, &dev.device_id) {
                            Ok(()) => {
                                event_bus.send(AgentEvent::DeviceApproved {
                                    device_id: dev.device_id.clone(),
                                });
                                state.set_flash(format!("Approved {}", short_id(&dev.device_id)));
                            }
                            Err(err) => state.set_flash(format!("Approve failed: {err}")),
                        },
                        None => state.set_flash("No pending devices"),
                    },
                    Err(err) => state.set_flash(format!("List failed: {err}")),
                },
                KeyCode::Char('l') => {
                    state.show_logs = !state.show_logs;
                }
                _ => {}
            }
        }
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(value.to_string())?;
    Ok(())
}

fn draw(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),  // steps + header
            Constraint::Min(14),    // QR + info OR logs
            Constraint::Length(3),  // footer (keybinds + last log)
        ])
        .split(area);

    render_header(f, outer[0], state);
    if state.show_logs {
        render_logs(f, outer[1], state);
    } else {
        render_body(f, outer[1], state);
    }
    render_footer(f, outer[2], state);
}

fn render_header(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " OxiRemote Host ",
            Style::default()
                .fg(Color::Rgb(108, 180, 255))
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    super::step_progress::render_steps(f, inner, &state.steps);
}

fn render_body(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_qr_panel(f, halves[0], state);
    render_info_panel(f, halves[1], state);
}

fn render_qr_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Pair a device ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let payload = state.qr_payload();
    let body = if state.tunnel_url.is_some() {
        match QrCode::new(payload.as_bytes()) {
            Ok(code) => code
                .render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Light)
                .light_color(unicode::Dense1x2::Dark)
                .quiet_zone(false)
                .build(),
            Err(_) => payload,
        }
    } else {
        payload
    };

    let para = Paragraph::new(body).alignment(Alignment::Center);
    f.render_widget(para, inner);
}

fn render_info_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Host Info ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let url = state
        .tunnel_url
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let devices_str = state.connected_devices.to_string();
    let otk_display = format_otk_status(state);
    let mut lines = vec![
        kv("App URL", &url),
        kv("One-Time Key", &otk_display),
        kv("Connected Devices", &devices_str),
    ];

    if state.ready_verifying() && !state.probe_log.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Verifying tunnel…",
            Style::default()
                .fg(Color::Rgb(108, 180, 255))
                .add_modifier(Modifier::BOLD),
        )));
        for entry in &state.probe_log {
            let mark = if entry.ok { "✓" } else { " " };
            let color = if entry.ok { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {mark} #{:<3}", entry.attempt),
                    Style::default().fg(color)),
                Span::styled(format!("→ {} ", entry.status),
                    Style::default().fg(Color::White)),
                Span::styled(format!("({}ms)", entry.elapsed_ms),
                    Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    lines.extend(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default()
                .fg(Color::Rgb(108, 180, 255))
                .add_modifier(Modifier::BOLD),
        )),
        action_line("c", "Copy tunnel URL"),
        action_line("k", "Copy one-time key"),
        action_line("r", "Regenerate one-time key"),
        action_line("a", "Approve next pending device"),
        action_line("l", "Toggle log view"),
        action_line("o", "Open tunnel URL in browser"),
        action_line("h", "Open host dashboard (/agent)"),
        action_line("q", "Exit TUI"),
    ]);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_logs(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Recent logs (l to hide) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line<'_>> = if state.log_history.is_empty() {
        vec![Line::from(Span::styled(
            "No log entries yet.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        state
            .log_history
            .iter()
            .map(|m| Line::from(Span::styled(m.clone(), Style::default().fg(Color::White))))
            .collect()
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn format_otk_status(state: &State) -> String {
    match (&state.otk_token, state.otk_expires_at) {
        (Some(token), Some(expires_at)) => {
            let now = now_secs();
            let remaining = expires_at - now;
            let last4: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            if remaining <= 0 {
                format!("····{last4}  expired (press r)")
            } else {
                let mins = remaining / 60;
                let secs = remaining % 60;
                format!("····{last4}  expires in {mins:02}:{secs:02}")
            }
        }
        _ => "— (press r to generate)".to_string(),
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn render_footer(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // Flash takes precedence so the operator sees confirmation of their last
    // hotkey press; falls back to the most recent log entry.
    let text = match (state.current_flash(), state.last_log.as_ref()) {
        (Some(flash), _) => format!(" {flash}"),
        (None, Some(msg)) => format!(" {msg}"),
        (None, None) => " OxiRemote is running. Keep this terminal open.".into(),
    };
    let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(para, area);
}

fn kv<'a>(k: &'a str, v: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {:<20}", k),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(v, Style::default().fg(Color::White)),
    ])
}

fn action_line<'a>(key: &'a str, label: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {key}  "),
            Style::default().fg(Color::Rgb(108, 180, 255)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::default().fg(Color::White)),
    ])
}
