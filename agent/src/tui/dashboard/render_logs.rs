// Log history panel: inline log stream toggled with `l`.
// `f` opens a filter-input band at the top; Enter pins the filter, Esc clears.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::state::{LogFilter, State};

pub(super) fn render_logs(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // When a filter is being edited or is active, split area: filter band on top.
    let show_filter_band = !matches!(state.log_filter, LogFilter::None);
    let (filter_area, log_area) = if show_filter_band {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    // Render filter band if active.
    if let Some(fa) = filter_area {
        render_filter_band(f, fa, state);
    }

    // Log panel block.
    let title = match &state.log_filter {
        LogFilter::None => " Recent logs (l to hide, f to filter) ".to_string(),
        LogFilter::Editing { .. } => " Recent logs — enter filter, Esc to clear ".to_string(),
        LogFilter::Active { pattern } => format!(" Recent logs — filter: {pattern} (f to change) "),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title.as_str());
    let inner = block.inner(log_area);
    f.render_widget(block, log_area);

    // Apply active filter to log entries (None when editing — show all while typing).
    let active_pattern = state.active_log_filter();

    let lines: Vec<Line<'_>> = if state.log_history.is_empty() {
        vec![Line::from(Span::styled(
            "No log entries yet.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        let filtered: Vec<_> = state
            .log_history
            .iter()
            .filter(|m| {
                if let Some(pat) = active_pattern {
                    m.to_lowercase().contains(&pat.to_lowercase())
                } else {
                    true
                }
            })
            .collect();

        if filtered.is_empty() {
            vec![Line::from(Span::styled(
                format!(
                    "No entries match \"{}\".",
                    active_pattern.unwrap_or("")
                ),
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            filtered
                .iter()
                .map(|m| Line::from(Span::styled(m.as_str(), Style::default().fg(Color::White))))
                .collect()
        }
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// One-line filter input band rendered above the log panel.
fn render_filter_band(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let (label, input, is_editing) = match &state.log_filter {
        LogFilter::Editing { input } => ("Filter: ", input.as_str(), true),
        LogFilter::Active { pattern } => ("Filter: ", pattern.as_str(), false),
        LogFilter::None => return,
    };

    let cursor = if is_editing { "_" } else { "" };
    let color = if is_editing { Color::Yellow } else { Color::Cyan };

    let line = Line::from(vec![
        Span::styled(label, Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{input}{cursor}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        if is_editing {
            Span::styled(
                "  [Enter] pin  [Esc] clear",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::raw("")
        },
    ]);
    f.render_widget(Paragraph::new(line), area);
}
