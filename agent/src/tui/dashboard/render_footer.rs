// Footer and onboarding hint renders.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::State;

pub(super) fn render_footer(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
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
