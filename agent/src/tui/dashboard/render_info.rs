// Host Info panel: App URL, OTK status, connected devices, probe log,
// recovery hint, and the hotkey actions list.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::{State, action_line, kv, kv_styled, now_secs};

pub(super) fn render_info_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    use ratatui::widgets::{Block, Borders};
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Host Info ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let url = if state.tunnel_down.is_some() {
        "tunnel down — connections will fail".to_string()
    } else {
        state.tunnel_url.clone().unwrap_or_else(|| "—".to_string())
    };
    let url_style = if state.tunnel_down.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::White)
    };
    let devices_str = state.connected_devices.to_string();
    let otk_display = format_otk_status(state);
    let mut lines = vec![
        kv_styled("App URL", &url, url_style),
        kv("One-Time Key", &otk_display),
        kv("Connected Devices", &devices_str),
    ];

    // Surface the recovery hint (carried on TunnelDown event payload, baked
    // into `tunnel_down` by State::apply) so the operator sees the actionable
    // next step right where the failure is — not buried in the log view.
    if let Some(reason) = &state.tunnel_down {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Recovery",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            reason.clone(),
            Style::default().fg(super::super::step_progress::BRAND),
        )));
    }

    if state.ready_verifying() && !state.probe_log.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Verifying tunnel…",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        )));
        for entry in &state.probe_log {
            let mark = if entry.ok { "✓" } else { " " };
            let color = if entry.ok { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {mark} #{:<3}", entry.attempt),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("→ {} ", entry.status),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("({}ms)", entry.elapsed_ms),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    let log_action_label = format!("Toggle log view ({} entries)", state.log_history.len());
    lines.extend(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        )),
        action_line("c", "Copy tunnel URL"),
        action_line("k", "Copy one-time key"),
        action_line("r", "Regenerate one-time key"),
        action_line("a", "Approve next pending device"),
        action_line("l", &log_action_label),
        action_line("o", "Open tunnel URL in browser"),
        action_line("h", "Open host dashboard (/agent)"),
        action_line("q", "Exit TUI"),
    ]);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

pub(super) fn format_otk_status(state: &State) -> String {
    match (&state.otk_token, state.otk_expires_at) {
        (Some(token), Some(expires_at)) => {
            let now = now_secs();
            let remaining = expires_at - now;
            let last4: String = token
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
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
