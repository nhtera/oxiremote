// Device manager list view and help overlay.
// Renders all trusted devices grouped: pending first (orange), then approved.
// Each row: index, platform icon, name/id, last-active age, status chip.
// `Enter` on a pending device → Approve; on approved → Revoke.
// `b`/Esc → back to dashboard.

use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use rusqlite::Connection;

use super::{State, now_secs};

// ── Device row model ──────────────────────────────────────────────────────────

/// A unified device row drawn from `trusted_devices`.
#[derive(Debug, Clone)]
pub(super) struct DeviceRow {
    pub(super) device_id: String,
    pub(super) label: String,
    pub(super) user_agent: Option<String>,
    pub(super) approval_status: String,
    pub(super) last_seen_at: i64,
    pub(super) device_name: Option<String>,
}

/// Load all non-revoked devices ordered pending-first, then by last_seen_at desc.
pub(super) fn list_devices(db_path: &PathBuf) -> Vec<DeviceRow> {
    let Ok(conn) = Connection::open(db_path) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT device_id, label, user_agent, approval_status, last_seen_at, device_name
         FROM trusted_devices
         WHERE revoked_at IS NULL
         ORDER BY
             CASE approval_status WHEN 'pending' THEN 0 ELSE 1 END,
             last_seen_at DESC",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |row| {
        Ok(DeviceRow {
            device_id: row.get(0)?,
            label: row.get(1)?,
            user_agent: row.get(2)?,
            approval_status: row.get(3)?,
            last_seen_at: row.get(4)?,
            device_name: row.get(5)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

// ── Platform icon ─────────────────────────────────────────────────────────────

/// Map a User-Agent string to a short platform glyph.
pub(crate) fn platform_icon_from_ua(ua: Option<&str>) -> &'static str {
    let Some(ua) = ua else { return "[?]" };
    let lower = ua.to_lowercase();
    if lower.contains("iphone") || lower.contains("ipad") {
        "[iOS]"
    } else if lower.contains("android") {
        "[And]"
    } else if lower.contains("mac") {
        "[Mac]"
    } else if lower.contains("windows") {
        "[Win]"
    } else if lower.contains("linux") {
        "[Lin]"
    } else {
        "[?]"
    }
}

// ── Render helpers ────────────────────────────────────────────────────────────

fn age_str(last_seen: i64) -> String {
    let now = now_secs();
    let secs = (now - last_seen).max(0) as u64;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn device_display_name(row: &DeviceRow) -> String {
    row.device_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(Some(row.label.as_str()))
        .unwrap_or(&row.device_id[..8.min(row.device_id.len())])
        .chars()
        .take(24)
        .collect()
}

// ── Main render ───────────────────────────────────────────────────────────────

pub(super) fn render_device_list(
    f: &mut Frame<'_>,
    area: Rect,
    state: &State,
    selected: usize,
) {
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::super::step_progress::BRAND_DIM))
        .title(Span::styled(
            " Device Manager  (b/Esc: back) ",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let devices = list_devices(&state.db_path);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    if devices.is_empty() {
        let para = Paragraph::new("  No devices yet.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, layout[0]);
    } else {
        let mut lines: Vec<Line> = Vec::with_capacity(devices.len() + 1);
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<3} {:<6} {:<24} {:<10} {}",
                "#", "Plat", "Name", "Last seen", "Status"
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::UNDERLINED),
        )));

        for (i, dev) in devices.iter().enumerate() {
            let icon = platform_icon_from_ua(dev.user_agent.as_deref());
            let name = device_display_name(dev);
            let age = age_str(dev.last_seen_at);
            let (status_label, status_color) = match dev.approval_status.as_str() {
                "pending" => ("PENDING", Color::Rgb(255, 140, 0)),
                "approved" => ("APPROVED", Color::Green),
                "rejected" => ("REJECTED", Color::Red),
                other => (other, Color::DarkGray),
            };
            let is_selected = i == selected;
            let row_style = if is_selected {
                Style::default()
                    .fg(super::super::step_progress::BRAND)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            let marker = if is_selected { "▶" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker} {:<2} {:<6} {:<24} {:<10} ", i + 1, icon, name, age),
                    row_style,
                ),
                Span::styled(status_label, Style::default().fg(status_color)),
            ]));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: true });
        f.render_widget(para, layout[0]);
    }

    // Bottom hint showing context-sensitive action.
    let hint = if let Some(dev) = devices.get(selected) {
        match dev.approval_status.as_str() {
            "pending" => "↑/↓ navigate  Enter: Approve  b/Esc: back",
            "approved" => "↑/↓ navigate  Enter: Revoke  b/Esc: back",
            _ => "↑/↓ navigate  b/Esc: back",
        }
    } else {
        "No devices  b/Esc: back"
    };
    let hint_para = Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray)))
        .alignment(Alignment::Center);
    f.render_widget(hint_para, layout[1]);
}

// ── Help overlay ──────────────────────────────────────────────────────────────

pub(super) fn render_help_overlay(f: &mut Frame<'_>, area: Rect) {
    let box_w = 56u16;
    let box_h = 22u16;
    let x = area.width.saturating_sub(box_w) / 2;
    let y = area.height.saturating_sub(box_h) / 2;
    let modal = Rect::new(x, y, box_w.min(area.width), box_h.min(area.height));

    f.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::super::step_progress::BRAND_DIM))
        .title(Span::styled(
            " Hotkey Legend  (? or Esc: close) ",
            Style::default()
                .fg(super::super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let content = inner.inner(Margin { horizontal: 1, vertical: 0 });

    let rows: &[(&str, &str)] = &[
        ("r", "Regenerate one-time key"),
        ("k", "Copy one-time key to clipboard"),
        ("c", "Copy tunnel URL to clipboard"),
        ("o", "Open tunnel URL in browser"),
        ("h", "Open host dashboard in browser"),
        ("d", "Device manager (approve/revoke)"),
        ("a", "Approve next pending device"),
        ("l", "Toggle log view"),
        ("b", "Return to splash menu"),
        ("?", "Toggle this help overlay"),
        ("q / Esc", "Exit TUI"),
        ("", ""),
        ("In Device Manager:", ""),
        ("↑ / k", "Move selection up"),
        ("↓ / j", "Move selection down"),
        ("Enter", "Approve (pending) / Revoke (approved)"),
        ("b / Esc", "Back to dashboard"),
    ];

    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, label)| {
            if key.is_empty() && label.is_empty() {
                return Line::from("");
            }
            if key.is_empty() {
                return Line::from(Span::styled(
                    *label,
                    Style::default()
                        .fg(super::super::step_progress::BRAND)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Line::from(vec![
                Span::styled(
                    format!("  {:<12}", key),
                    Style::default()
                        .fg(super::super::step_progress::BRAND)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*label, Style::default().fg(Color::Gray)),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), content);
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_icon_ios() {
        assert_eq!(platform_icon_from_ua(Some("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)")), "[iOS]");
        assert_eq!(platform_icon_from_ua(Some("iPad Safari")), "[iOS]");
    }

    #[test]
    fn platform_icon_android() {
        assert_eq!(platform_icon_from_ua(Some("Mozilla/5.0 (Linux; Android 14)")), "[And]");
    }

    #[test]
    fn platform_icon_mac() {
        assert_eq!(platform_icon_from_ua(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")), "[Mac]");
    }

    #[test]
    fn platform_icon_windows() {
        assert_eq!(platform_icon_from_ua(Some("Mozilla/5.0 (Windows NT 10.0; Win64)")), "[Win]");
    }

    #[test]
    fn platform_icon_linux() {
        assert_eq!(platform_icon_from_ua(Some("curl/8.0 (x86_64-pc-linux-gnu)")), "[Lin]");
    }

    #[test]
    fn platform_icon_unknown() {
        assert_eq!(platform_icon_from_ua(Some("SomeRandomBot/1.0")), "[?]");
        assert_eq!(platform_icon_from_ua(None), "[?]");
    }
}
