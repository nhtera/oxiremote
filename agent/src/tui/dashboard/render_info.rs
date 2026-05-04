// Host Info panel: App URL, OTK status, connected devices, probe log,
// recovery hint, and the hotkey actions list.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::{State, action_line, kv, kv_styled, now_secs};

/// Format an OTK token as groups of 4 chars separated by hyphens.
/// A 16-char token → `XXXX-XXXX-XXXX-XXXX`. Shorter tokens are output as-is.
pub(super) fn format_otk_grouped(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 8 {
        return token.to_string();
    }
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

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
        // Show verifying elapsed time with reassurance copy.
        let elapsed_line = format_verifying_elapsed(state);
        lines.push(Line::from(Span::styled(
            elapsed_line,
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

    // Verifying elapsed timer when we have a start timestamp.
    if let Some(started) = state.verifying_started {
        let secs = started.elapsed().as_secs();
        if secs > 0 {
            lines.push(Line::from(""));
            let hint = verifying_hint(secs);
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let log_action_label = format_log_action_label(state.log_history.len());
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
        action_line("d", "Device manager"),
        action_line("l", &log_action_label),
        action_line("f", "Filter log view (when logs visible)"),
        action_line("t", "Toggle Remote Desktop service"),
        action_line("s", "Toggle autostart on login"),
        action_line("R", "Rotate permanent API key (confirm required)"),
        action_line("o", "Open tunnel URL in browser"),
        action_line("h", "Open host dashboard (/agent)"),
        action_line("?", "Help / hotkey legend"),
        action_line("b", "Back to menu"),
        action_line("q", "Exit TUI"),
    ]);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Action-line label for the "toggle log view" hotkey, including buffer size.
/// Surfaced separately so the operator knows there's history worth flipping
/// to (action_line_includes_log_count test pins this contract).
pub(super) fn format_log_action_label(count: usize) -> String {
    format!("Toggle log view ({count} entries)")
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
            // Show full token grouped for plain-text readability (SSH operators
            // can read it without clipboard access).
            let grouped = format_otk_grouped(token);
            if remaining <= 0 {
                format!("{grouped}  (····{last4}, expired — press r)")
            } else {
                let mins = remaining / 60;
                let secs = remaining % 60;
                format!("{grouped}  expires in {mins:02}:{secs:02}")
            }
        }
        _ => "— (press r to generate)".to_string(),
    }
}

/// Verifying elapsed header line for the probe log section.
pub(super) fn format_verifying_elapsed(state: &State) -> String {
    if let Some(started) = state.verifying_started {
        let secs = started.elapsed().as_secs();
        return format!("Verifying… {}s elapsed", secs);
    }
    "Verifying tunnel…".to_string()
}

/// Reassurance copy shown below the probe log while Verifying is in progress.
pub(super) fn verifying_hint(secs: u64) -> String {
    if secs >= 90 {
        "still connecting, Cloudflare DNS propagating".to_string()
    } else if secs >= 30 {
        "usually 30–90 s, nothing is wrong".to_string()
    } else {
        String::new()
    }
}
