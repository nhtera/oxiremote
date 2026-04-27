// Log history panel: inline log stream toggled with `l`.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::State;

pub(super) fn render_logs(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
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
