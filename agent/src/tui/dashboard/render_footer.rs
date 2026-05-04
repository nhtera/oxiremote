// Footer and onboarding hint renders.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::State;

pub(super) fn render_footer(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // Split footer: top line = flash/log message, bottom line = hotkey hints.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    // Flash takes precedence so the operator sees confirmation of their last
    // hotkey press; falls back to the most recent log entry.
    let text = match (state.current_flash(), state.last_log.as_ref()) {
        (Some(flash), _) => format!(" {flash}"),
        (None, Some(msg)) => format!(" {msg}"),
        (None, None) => " OxiRemote is running. Keep this terminal open.".into(),
    };
    let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(para, rows[0]);

    // Always-visible hotkey hint strip.
    // Includes the new t/s/R keys for operator parity with the WebUI.
    let hints = Line::from(vec![
        hint_span("r", "regen-otk"),
        sep_span(),
        hint_span("d", "devices"),
        sep_span(),
        hint_span("l", "logs"),
        sep_span(),
        hint_span("t", "toggle-rd"),
        sep_span(),
        hint_span("s", "autostart"),
        sep_span(),
        hint_span("R", "rotate-key"),
        sep_span(),
        hint_span("c", "copy URL"),
        sep_span(),
        hint_span("?", "help"),
        sep_span(),
        hint_span("b", "menu"),
        sep_span(),
        hint_span("q", "quit"),
    ]);
    f.render_widget(
        Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
        rows[1],
    );
}

fn hint_span(key: &'static str, label: &'static str) -> Span<'static> {
    Span::raw(format!(" {key}:{label}"))
}

fn sep_span() -> Span<'static> {
    Span::styled("  ", Style::default().fg(Color::DarkGray))
}

pub(super) fn render_onboarding_hint(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let hint = state.onboarding_hint();
    let color = if state.tunnel_down.is_some() {
        Color::Red
    } else {
        super::super::step_progress::BRAND
    };
    let para = Paragraph::new(Line::from(Span::styled(
        hint,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    f.render_widget(para, area);
}
