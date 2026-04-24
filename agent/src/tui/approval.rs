// Full-screen device-approval takeover. Triggered by AgentEvent::DevicePending.
// Posts approve/reject back to the localhost `/api/agent/approvals/*` endpoints
// (stub in Phase 01 — Phase 02 adds the real handlers). Keeps the TUI
// synchronous by blocking on a tiny reqwest call via ureq-style blocking
// pattern: spawn a short-lived thread.

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::events::AgentEvent;

type Term = Terminal<CrosstermBackend<io::Stdout>>;

pub fn run_approval(term: &mut Term, event: &AgentEvent) -> Result<()> {
    let AgentEvent::DevicePending {
        device_id,
        ip,
        ua_parsed,
        ..
    } = event
    else {
        return Ok(());
    };

    loop {
        term.draw(|f| draw(f, device_id, ip, ua_parsed))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        call_decision(device_id, "approve");
                        return Ok(());
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        call_decision(device_id, "reject");
                        return Ok(());
                    }
                    _ => {}
                }
            }
    }
}

fn call_decision(device_id: &str, decision: &str) {
    // Fire-and-forget POST. Phase 02 implements the handler; until then this
    // returns 404, which we intentionally ignore — the UI flow is already
    // complete from the TUI side.
    let url = format!(
        "http://localhost:8787/api/agent/approvals/{}/{}",
        device_id, decision
    );
    std::thread::spawn(move || {
        let _ = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .and_then(|c| c.post(&url).send());
    });
}

fn draw(f: &mut ratatui::Frame<'_>, device_id: &str, ip: &str, ua: &str) {
    let area = f.area();
    f.render_widget(Clear, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(14)) / 2),
            Constraint::Length(14),
            Constraint::Min(0),
        ])
        .split(area);
    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(64),
            Constraint::Min(0),
        ])
        .split(layout[1]);

    let modal = inner[1];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Double)
        .border_style(Style::default().fg(Color::Rgb(255, 140, 0)))
        .title(Span::styled(
            " Approve New Device? ",
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, modal);

    let content = modal.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let short_id: String = device_id.chars().take(12).collect();
    let lines = vec![
        row("Device", &short_id),
        row("IP", ip),
        row("User-Agent", ua),
        Line::from(""),
        Line::from(Span::styled(
            "  y  Approve",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  n  Reject",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Esc  Cancel (defaults to Reject)",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Left),
        content,
    );
}

fn row<'a>(k: &'a str, v: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{:<14}", k), Style::default().fg(Color::Gray)),
        Span::styled(v, Style::default().fg(Color::White)),
    ])
}

// Silence unused-import lint for Rect when the only use is inside a generic
// that the compiler already infers.
#[allow(dead_code)]
fn _type_hint(_: Rect) {}
