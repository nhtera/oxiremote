// Inline step-progress rows (done/active/pending). Used in the dashboard
// during the "Prepare → Listening → Tunnel up → Ready" boot transition.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::events::StepStatus;

pub const BRAND: Color = Color::Rgb(255, 122, 64);
pub const BRAND_DIM: Color = Color::Rgb(168, 82, 44);

pub struct Step {
    pub name: String,
    pub status: StepStatus,
    pub sub: Option<String>,
}

/// Same as `render_steps` but accepts a slice of references — used by the
/// narrow-terminal compact path in `dashboard.rs` which filters a subset.
pub fn render_steps_refs(f: &mut Frame<'_>, area: Rect, steps: &[&Step]) {
    let owned: Vec<_> = steps
        .iter()
        .map(|s| Step {
            name: s.name.clone(),
            status: s.status,
            sub: s.sub.clone(),
        })
        .collect();
    render_steps(f, area, &owned);
}

pub fn render_steps(f: &mut Frame<'_>, area: Rect, steps: &[Step]) {
    let total_w = area.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(steps.len());
    for step in steps {
        let (icon, name_style, badge) = match step.status {
            StepStatus::Done => (
                "●",
                Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD),
                Some(("Done", Style::default().fg(Color::Rgb(74, 222, 128)))),
            ),
            StepStatus::Active => (
                "●",
                Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
                Some(("Running", Style::default().fg(BRAND).add_modifier(Modifier::BOLD))),
            ),
            StepStatus::Pending => (
                "○",
                Style::default().fg(Color::DarkGray),
                None,
            ),
        };

        let icon_span = Span::styled(icon, name_style);
        let name_span = Span::styled(step.name.as_str(), name_style);
        let sub_span = step.sub.as_ref().map(|s| Span::styled(
            format!("  {s}"),
            Style::default().fg(Color::Gray),
        ));

        let used_left = 1 + 2 + step.name.len()
            + step.sub.as_ref().map(|s| s.len() + 2).unwrap_or(0);
        let badge_label_w = badge.map(|(t, _)| t.len()).unwrap_or(0);
        let pad = total_w
            .saturating_sub(used_left)
            .saturating_sub(badge_label_w);

        let mut spans = vec![icon_span, Span::raw("  "), name_span];
        if let Some(s) = sub_span { spans.push(s); }
        if let Some((label, style)) = badge {
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(label, style));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), area);
}
