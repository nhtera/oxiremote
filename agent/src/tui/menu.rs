// Splash menu — fixed options, arrow-key nav, Enter to confirm. If a newer
// version is available it appears as the first item (yellow text).

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuChoice {
    OpenWebUi,
    TerminalUi,
    UpdateAvailable(String), // version string
    Exit,
}

struct MenuItem {
    label: String,
    choice: MenuChoice,
    /// Yellow warning style for update items.
    highlight: bool,
}

/// Splash title rendered in the menu's bordered box. The version stamp lives
/// here so users can confirm the build at a glance — every bug report starts
/// with "what version?". Compile-time `env!` ensures a rebuild ships the bump.
pub(super) fn splash_title() -> String {
    format!("  OxiRemote v{}  ", env!("CARGO_PKG_VERSION"))
}

fn build_items(update_version: Option<&str>) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = Vec::with_capacity(4);
    if let Some(ver) = update_version {
        items.push(MenuItem {
            label: format!("Update to v{ver} available"),
            choice: MenuChoice::UpdateAvailable(ver.to_string()),
            highlight: true,
        });
    }
    items.push(MenuItem {
        label: "Open Web UI (background)".into(),
        choice: MenuChoice::OpenWebUi,
        highlight: false,
    });
    items.push(MenuItem {
        label: "Terminal UI".into(),
        choice: MenuChoice::TerminalUi,
        highlight: false,
    });
    items.push(MenuItem {
        label: "Exit".into(),
        choice: MenuChoice::Exit,
        highlight: false,
    });
    items
}

pub fn run_menu(term: &mut Term, update_version: Option<&str>) -> Result<MenuChoice> {
    let items = build_items(update_version);

    // Default selection: "Open Web UI (background)". Hitting Enter on the
    // splash gives the operator a menu-bar tray + browser dashboard, which
    // is the right default for a remote-access tool. Arrow to "Terminal UI"
    // for the in-terminal dashboard when desired.
    let default_label = "Open Web UI (background)";
    let mut selected: usize = items
        .iter()
        .position(|i| i.label == default_label)
        .unwrap_or(0);

    loop {
        term.draw(|f| draw(f, &items, selected))?;
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
                        if selected + 1 < items.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(MenuChoice::Exit),
                    KeyCode::Enter => return Ok(items[selected].choice.clone()),
                    _ => {}
                }
            }
    }
}

fn draw(f: &mut ratatui::Frame<'_>, items: &[MenuItem], selected: usize) {
    let area = f.area();
    // Expand box height by 1 for each extra item beyond the 3 baseline options.
    let extra = items.len().saturating_sub(3) as u16;
    let box_height = 14 + extra;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(box_height)) / 2),
            Constraint::Length(box_height),
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
        .border_style(Style::default().fg(super::step_progress::BRAND_DIM))
        .title(Span::styled(
            splash_title(),
            Style::default()
                .fg(super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block.clone(), inner[1]);

    let content_area = inner[1].inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let mut lines: Vec<Line> = Vec::with_capacity(items.len() + 3);
    lines.push(Line::from(vec![
        Span::styled("▸ ", Style::default().fg(super::step_progress::BRAND)),
        Span::styled(
            "Your dev box, in your pocket.",
            Style::default().fg(Color::Gray),
        ),
    ]));
    lines.push(Line::from(""));
    for (i, item) in items.iter().enumerate() {
        let marker = if i == selected { "★ " } else { "  " };
        let style = if item.highlight {
            // Update item is always rendered in yellow regardless of selection.
            if i == selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            }
        } else if i == selected {
            Style::default()
                .fg(super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(item.label.as_str(), style),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_includes_version() {
        let title = splash_title();
        assert!(title.contains("OxiRemote"));
        assert!(
            title.contains(env!("CARGO_PKG_VERSION")),
            "splash title {title:?} should embed the package version"
        );
    }

    #[test]
    fn build_items_prepends_update_when_available() {
        let items = build_items(Some("1.2.3"));
        assert!(items[0].highlight, "update item must be highlighted");
        assert!(matches!(items[0].choice, MenuChoice::UpdateAvailable(_)));
        assert!(items[0].label.contains("1.2.3"));
        // No update → first item is OpenWebUi (no update banner).
        let plain = build_items(None);
        assert!(matches!(plain[0].choice, MenuChoice::OpenWebUi));
    }
}
