// Full-screen device-approval takeover. Triggered by AgentEvent::DevicePending.
// Posts approve/reject back to the localhost `/api/agent/approvals/*` endpoints.
// Keeps the TUI synchronous by blocking on a tiny reqwest call via ureq-style
// blocking pattern: spawn a short-lived thread.

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

use crate::auth::{PAIRING_TTL_SECS, browser_from_ua, platform_from_ua};
use crate::events::AgentEvent;

/// Capitalize the first letter of a coarse-platform code (e.g. "ios" → "iOS",
/// "macos" → "macOS"). Used only by the approval modal to render
/// `Platform · Browser` instead of a raw UA string.
fn pretty_platform(p: &str) -> String {
    match p {
        "ios" => "iOS".into(),
        "macos" => "macOS".into(),
        "android" => "Android".into(),
        "windows" => "Windows".into(),
        "linux" => "Linux".into(),
        other => other.into(),
    }
}

type Term = Terminal<CrosstermBackend<io::Stdout>>;

pub fn run_approval(term: &mut Term, event: &AgentEvent) -> Result<()> {
    let AgentEvent::DevicePending {
        device_id,
        ip,
        ua_parsed,
        first_seen,
        ..
    } = event
    else {
        return Ok(());
    };

    let expires_at = *first_seen + PAIRING_TTL_SECS;

    loop {
        let remaining = expires_at - now_secs();
        // TTL exhausted — auto-dismiss without writing a decision so the
        // server-side pairing record can age out via its own expiry path.
        if remaining <= 0 {
            return Ok(());
        }
        term.draw(|f| draw(f, device_id, ip, ua_parsed, remaining))?;
        // 1 s tick — render-driven so the countdown updates even when no key
        // is pressed. Falls through to redraw on either key or timeout.
        if event::poll(Duration::from_secs(1))?
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

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn call_decision(device_id: &str, decision: &str) {
    // Fire-and-forget POST. We intentionally ignore errors — the UI flow is
    // already complete from the TUI side.
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

fn draw(f: &mut ratatui::Frame<'_>, device_id: &str, ip: &str, ua: &str, remaining: i64) {
    let area = f.area();
    f.render_widget(Clear, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(15)) / 2),
            Constraint::Length(15),
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
    // Derive Platform · Browser from the UA so the operator can decide at a
    // glance instead of squinting at a long UA string.
    let platform = platform_from_ua(ua).map(|p| pretty_platform(&p));
    let browser = browser_from_ua(ua);
    let parsed = match (platform, browser) {
        (Some(p), Some(b)) => format!("{p} · {b}"),
        (Some(p), None) => p,
        (None, Some(b)) => b,
        (None, None) => ua.chars().take(48).collect(),
    };
    // Countdown — red within the last minute so the operator sees urgency.
    let mins = remaining / 60;
    let secs = remaining % 60;
    let countdown_style = if remaining < 60 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let lines = vec![
        row("Device", &short_id),
        row("IP", ip),
        row("Client", &parsed),
        Line::from(vec![
            Span::styled(format!("{:<14}", "Expires in"), Style::default().fg(Color::Gray)),
            Span::styled(format!("{}:{:02}", mins, secs), countdown_style),
        ]),
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
