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

pub struct Step {
    pub name: String,
    pub status: StepStatus,
    pub sub: Option<String>,
}

pub fn render_steps(f: &mut Frame<'_>, area: Rect, steps: &[Step]) {
    let mut lines: Vec<Line> = Vec::with_capacity(steps.len());
    for step in steps {
        let (icon, style) = match step.status {
            StepStatus::Done => (
                "✓",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            StepStatus::Active => (
                "◌",
                Style::default()
                    .fg(Color::Rgb(255, 140, 0))
                    .add_modifier(Modifier::BOLD),
            ),
            StepStatus::Pending => ("○", Style::default().fg(Color::DarkGray)),
        };
        let mut spans = vec![
            Span::styled(icon, style),
            Span::raw("  "),
            Span::styled(step.name.as_str(), style),
        ];
        if let Some(sub) = &step.sub {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                sub.as_str(),
                Style::default().fg(Color::Gray),
            ));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), area);
}
