// Main TUI dashboard. Shows tunnel URL QR, OTK placeholder (Phase 02 wires
// the real token), host info, and an action menu. Subscribes to the event
// bus so tunnel/device state updates live-refresh without polling.

use std::io;
use std::sync::Arc;
use std::time::Duration;

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

type Term = Terminal<CrosstermBackend<io::Stdout>>;

struct State {
    tunnel_url: Option<String>,
    steps: Vec<super::step_progress::Step>,
    connected_devices: usize,
    last_log: Option<String>,
}

impl State {
    fn new() -> Self {
        use super::step_progress::Step;
        Self {
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
        }
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
                    s.status = StepStatus::Done;
                    s.sub = Some("waiting for devices".into());
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
            }
            _ => {}
        }
    }
}

pub fn run_dashboard(term: &mut Term, event_bus: Arc<EventBus>) -> Result<()> {
    let mut rx = event_bus.subscribe();
    let mut state = State::new();

    // Seed from current tunnel URL if it already resolved before we subscribed.
    // (In practice the event bus also buffers the last TunnelUrlChanged.)

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
            && let Event::Key(key) = event::read()? {
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
                    _ => {}
                }
            }
    }
}

fn draw(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),  // steps + header
            Constraint::Min(14),    // QR + info
            Constraint::Length(3),  // footer (keybinds + last log)
        ])
        .split(area);

    render_header(f, outer[0], state);
    render_body(f, outer[1], state);
    render_footer(f, outer[2], state);
}

fn render_header(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " OxiRemote Host ",
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
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
        .title(" Tunnel URL ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let body = match &state.tunnel_url {
        Some(url) => {
            let code = QrCode::new(url.as_bytes()).ok();
            match code {
                Some(c) => c
                    .render::<unicode::Dense1x2>()
                    .dark_color(unicode::Dense1x2::Light)
                    .light_color(unicode::Dense1x2::Dark)
                    .quiet_zone(false)
                    .build(),
                None => url.clone(),
            }
        }
        None => "Tunnel not ready yet…".to_string(),
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
    let lines = vec![
        kv("App URL", &url),
        kv("One-Time Key", "— (Phase 02)"),
        kv("Connected Devices", &devices_str),
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  o  Open tunnel URL in browser",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  h  Open host dashboard (/agent)",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  q  Exit TUI",
            Style::default().fg(Color::White),
        )),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_footer(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let text = match &state.last_log {
        Some(msg) => format!(" {}", msg),
        None => " OxiRemote is running. Keep this terminal open.".into(),
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
