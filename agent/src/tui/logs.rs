// Scrollable log viewer widget. Not currently routed from run_tui's splash
// menu — reserved for Phase 05's `/agent/logs` feature. Keeps the widget
// testable in isolation.
#![allow(dead_code)]

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::events::{AgentEvent, LogLevel};

pub struct LogLine {
    pub level: LogLevel,
    pub module: String,
    pub msg: String,
}

pub fn ingest(buf: &mut Vec<LogLine>, event: &AgentEvent, cap: usize) {
    if let AgentEvent::LogEntry {
        level, module, msg, ..
    } = event
    {
        buf.push(LogLine {
            level: *level,
            module: module.clone(),
            msg: msg.clone(),
        });
        if buf.len() > cap {
            let drop = buf.len() - cap;
            buf.drain(0..drop);
        }
    }
}

pub fn render_logs(f: &mut Frame<'_>, area: Rect, buf: &[LogLine]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Logs ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible = buf
        .iter()
        .rev()
        .take(inner.height as usize)
        .collect::<Vec<_>>()
        .into_iter()
        .rev();

    let lines: Vec<Line> = visible
        .map(|l| {
            let level_color = match l.level {
                LogLevel::Info => Color::Gray,
                LogLevel::Warn => Color::Yellow,
                LogLevel::Error => Color::Red,
            };
            let tag = match l.level {
                LogLevel::Info => "INFO ",
                LogLevel::Warn => "WARN ",
                LogLevel::Error => "ERROR",
            };
            Line::from(vec![
                Span::styled(tag, Style::default().fg(level_color)),
                Span::raw("  "),
                Span::styled(
                    format!("{:<14}", l.module),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(" "),
                Span::styled(l.msg.as_str(), Style::default().fg(Color::White)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
