// Header render functions: full step-checklist header (pre-Ready) and
// single-line compact header (post-Ready).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::State;

pub(super) fn render_header(f: &mut ratatui::Frame<'_>, area: Rect, state: &State, wide: bool) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "OxiRemote",
                Style::default()
                    .fg(super::super::step_progress::BRAND)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                if state.is_ready() { "host" } else { "setting up connection" },
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
        ]));
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    if wide {
        // Full 5-step checklist.
        super::super::step_progress::render_steps(f, inner, &state.steps, state.spinner_frame);
    } else {
        // Narrow terminal — compact 3-row summary: Server | Tunnel | Ready.
        let compact: Vec<_> = state
            .steps
            .iter()
            .filter(|s| matches!(s.name.as_str(), "Preparing" | "Tunneling" | "Ready"))
            .collect();
        super::super::step_progress::render_steps_refs(f, inner, &compact, state.spinner_frame);
    }
}

/// Single-line header shown once the tunnel is Ready.
/// Format: `● OxiRemote  ·  <url>` where URL is truncated with ellipsis if needed.
pub(super) fn render_compact_header(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let url_raw = state.tunnel_url.as_deref().unwrap_or("—");

    // Prefix spans: green dot + brand name + dim separator
    let prefix_spans = vec![
        Span::styled(
            "● ",
            Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "OxiRemote",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  ·  ",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ),
    ];
    // "● " (2) + "OxiRemote" (9) + "  ·  " (5) = 16 chars prefix width
    let prefix_w: usize = 16;
    let avail = (area.width as usize).saturating_sub(prefix_w);

    let url_display = if url_raw.len() > avail && avail > 3 {
        format!("{}…", &url_raw[..avail.saturating_sub(1)])
    } else {
        url_raw.to_string()
    };

    let mut spans = prefix_spans;
    spans.push(Span::styled(url_display, Style::default().fg(Color::White)));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
