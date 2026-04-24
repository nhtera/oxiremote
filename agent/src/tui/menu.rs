// Splash menu — three fixed options, arrow-key nav, Enter to confirm.

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuChoice {
    OpenWebUi,
    TerminalUi,
    Exit,
}

const ITEMS: &[(&str, MenuChoice)] = &[
    ("Open Web UI (background)", MenuChoice::OpenWebUi),
    ("Terminal UI", MenuChoice::TerminalUi),
    ("Exit", MenuChoice::Exit),
];

pub fn run_menu(term: &mut Term) -> Result<MenuChoice> {
    let mut selected: usize = 1; // default to Terminal UI
    loop {
        term.draw(|f| draw(f, selected))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected + 1 < ITEMS.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(MenuChoice::Exit),
                    KeyCode::Enter => return Ok(ITEMS[selected].1),
                    _ => {}
                }
            }
    }
}

fn draw(f: &mut ratatui::Frame<'_>, selected: usize) {
    let area = f.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(14)) / 2),
            Constraint::Length(14),
            Constraint::Min(0),
        ])
        .split(area);

    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(56),
            Constraint::Min(0),
        ])
        .split(layout[1]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 140, 0)))
        .title(Span::styled(
            " OxiRemote ",
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block.clone(), inner[1]);

    let content_area = inner[1].inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let mut lines: Vec<Line> = Vec::with_capacity(ITEMS.len() + 3);
    lines.push(Line::from(Span::styled(
        "Self-hosted remote agent",
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(""));
    for (i, (label, _)) in ITEMS.iter().enumerate() {
        let marker = if i == selected { "▸ " } else { "  " };
        let style = if i == selected {
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(*label, style),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ navigate  ↵ select  q exit",
        Style::default().fg(Color::DarkGray),
    )));

    let para = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(para, content_area);
}
