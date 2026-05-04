// Main TUI dashboard. Shows the pairing QR (encoding `<tunnel>/login?k=<otk>`
// so a phone scan auto-fills the OTK), live OTK + countdown, host info, and a
// hotkey panel. Subscribes to the event bus so tunnel/device/OTK state updates
// live-refresh without polling.

pub(super) mod render_device_list;
pub(super) mod render_footer;
pub(super) mod render_header;
pub(super) mod render_info;
pub(super) mod render_logs;
pub(super) mod render_qr;

mod event_handler;
mod key;
mod state;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::approval;
use crate::events::{AgentEvent, EventBus};

// Re-export state types so render submodules can access them via `super::State`
// (render_* are children of this mod.rs; pub(super) makes them visible there).
pub(super) use state::{ConfirmModal, State, ViewState, now_secs};
// Used in test section and by dashboard submodule key.rs.
#[allow(unused_imports)]
pub(super) use state::{LogFilter, ProbeEntry};

use render_device_list::render_device_list;
use render_footer::{render_footer, render_onboarding_hint};
use render_header::{render_compact_header, render_header};
use render_info::render_info_panel;
use render_logs::render_logs;
use render_qr::render_qr_panel;

type Term = Terminal<CrosstermBackend<io::Stdout>>;

pub fn run_dashboard(
    term: &mut Term,
    event_bus: Arc<EventBus>,
    db_path: PathBuf,
    discovery_url: Option<String>,
    discovery_temp_key: Arc<StdRwLock<Option<String>>>,
) -> Result<()> {
    let mut rx = event_bus.subscribe();
    let mut state = State::new(db_path, discovery_url, discovery_temp_key);

    // Hydrate from the bus's tunnel snapshot before starting the event loop.
    // The TUI subscribes lazily (after the menu picks "Terminal UI"), so events
    // that fired during boot — TunnelStepChanged / TunnelUrlChanged / Ready —
    // would otherwise be lost: tokio broadcast has no replay. Without this, a
    // user who lingers in the menu past the ~8s health-probe window enters a
    // dashboard stuck on the onboarding view forever.
    let snap = event_bus.snapshot();
    if let Some(url) = snap.url {
        state.tunnel_url = Some(url);
        state.cascade_done_through("Tunneling");
    }
    if let Some(step_ev) = snap.latest_step {
        state.apply(&step_ev);
    }
    if let Some(reason) = snap.down_reason {
        state.apply(&AgentEvent::TunnelDown { reason, recovery_hint: None });
    }

    loop {
        term.draw(|f| draw(f, &state))?;
        state.spinner_frame = state.spinner_frame.wrapping_add(1) % 10;

        // Drain buffered events without blocking so redraws stay responsive.
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::DevicePending { .. } = &event {
                approval::run_approval(term, &event)?;
                continue;
            }
            state.apply(&event);
        }

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Route keys differently depending on active sub-view.
            match &state.view_state.clone() {
                ViewState::DeviceList { selected } => {
                    key::handle_device_list_key(
                        key.code,
                        *selected,
                        &mut state,
                        &event_bus,
                    );
                    continue;
                }
                ViewState::Dashboard => {}
            }
            // Dashboard-level keys — quit signal propagates out of loop.
            if key::handle_dashboard_key(key.code, &mut state, &event_bus) {
                return Ok(());
            }
        }
    }
}

// ── Shared helpers used by multiple render submodules ─────────────────────────

pub(super) fn kv<'a>(k: &'a str, v: &'a str) -> Line<'a> {
    kv_styled(k, v, Style::default().fg(Color::White))
}

pub(super) fn kv_styled<'a>(k: &'a str, v: &'a str, value_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {:<20}", k), Style::default().fg(Color::DarkGray)),
        Span::styled(v, value_style),
    ])
}

pub(super) fn action_line<'a>(key: &'a str, label: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {key}  "),
            Style::default()
                .fg(super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::default().fg(Color::Gray)),
    ])
}

/// Pluck the first `https://...` token from free-form text. Used to recover
/// the tunnel URL from a TunnelStepChanged event's `info` field when the TUI
/// missed the original `TunnelUrlChanged` event (subscription happens after
/// the menu, so early events go to /dev/null). Stops at the first
/// whitespace, paren, or quote so suffixes like "(probe inconclusive)"
/// don't end up in the URL.
pub(super) fn extract_tunnel_url(info: &str) -> Option<String> {
    let start = info.find("https://")?;
    let tail = &info[start..];
    let end = tail
        .find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '"' | '\'' | '<' | '>'))
        .unwrap_or(tail.len());
    let url = &tail[..end];
    if url.len() > "https://".len() {
        Some(url.to_string())
    } else {
        None
    }
}

// ── Draw dispatch ─────────────────────────────────────────────────────────────

fn draw(f: &mut ratatui::Frame<'_>, state: &State) {
    // Device list overrides the normal dashboard entirely.
    if let ViewState::DeviceList { selected } = state.view_state {
        render_device_list(f, f.area(), state, selected);
        return;
    }
    if state.is_ready() {
        draw_dashboard(f, state);
    } else {
        draw_onboarding(f, state);
    }
    // Help overlay rendered on top of whatever view is active.
    if state.help_overlay {
        render_device_list::render_help_overlay(f, f.area());
    }
    // Confirm modal rendered on top of everything.
    if state.confirm_modal != ConfirmModal::None {
        render_confirm_modal(f, f.area(), &state.confirm_modal);
    }
}

fn draw_dashboard(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    let wide = area.width >= 80;

    if state.is_ready() {
        // Collapsed single-line header: frees vertical space for QR + info panes.
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // compact status line
                Constraint::Min(14),   // QR + info OR logs
                Constraint::Length(3), // footer
            ])
            .split(area);

        render_compact_header(f, outer[0], state);
        if state.show_logs {
            render_logs(f, outer[1], state);
        } else {
            render_body(f, outer[1], state);
        }
        render_footer(f, outer[2], state);
    } else {
        // Onboarding path — full 5-step checklist header.
        let header_h = if wide { 8u16 } else { 6 };
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h), // steps + header
                Constraint::Min(14),          // QR + info OR logs
                Constraint::Length(3),        // footer (keybinds + last log)
            ])
            .split(area);

        render_header(f, outer[0], state, wide);
        if state.show_logs {
            render_logs(f, outer[1], state);
        } else {
            render_body(f, outer[1], state);
        }
        render_footer(f, outer[2], state);
    }
}

// Onboarding view: step checklist only — no QR (unscannable until Ready),
// no Host Info (most fields aren't real until Ready), no Actions list (most
// keybinds are no-ops pre-Ready). The operator sees just what matters: the
// 5-step progress and a one-line live status hint. `q` (quit) and `l`
// (toggle logs) still work via the global key handler.
fn draw_onboarding(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    if state.show_logs {
        let wide = area.width >= 80;
        let header_h = if wide { 8u16 } else { 6 };
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h),
                Constraint::Min(14),
                Constraint::Length(3),
            ])
            .split(area);
        render_header(f, outer[0], state, wide);
        render_logs(f, outer[1], state);
        render_footer(f, outer[2], state);
        return;
    }

    let wide = area.width >= 80;
    let header_h = if wide { 8u16 } else { 6 };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h), // step checklist
            Constraint::Min(0),           // hint area (large spacer; centers the message)
            Constraint::Length(2),        // probe-info / hint footer
            Constraint::Length(2),        // bottom spacer
        ])
        .split(area);

    render_header(f, outer[0], state, wide);
    render_onboarding_hint(f, outer[2], state);
}

fn render_body(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_qr_panel(f, halves[0], state);
    render_info_panel(f, halves[1], state);
}

/// Confirm modal overlay — centred, asks for y/n confirmation.
fn render_confirm_modal(f: &mut ratatui::Frame<'_>, area: Rect, modal: &ConfirmModal) {
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let title = match modal {
        ConfirmModal::RotatePermanentKey => " Rotate Permanent Key ",
        ConfirmModal::None => return,
    };
    let body = match modal {
        ConfirmModal::RotatePermanentKey => {
            "All paired devices will be disconnected\nand must re-pair after rotation.\n\n  y = confirm    n/Esc = cancel"
        }
        ConfirmModal::None => return,
    };

    let box_w = 52u16;
    let box_h = 8u16;
    let x = area.width.saturating_sub(box_w) / 2;
    let y = area.height.saturating_sub(box_h) / 2;
    let modal_rect =
        Rect::new(x, y, box_w.min(area.width), box_h.min(area.height));

    f.render_widget(Clear, modal_rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(255, 140, 0)))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(modal_rect);
    f.render_widget(block, modal_rect);
    f.render_widget(
        Paragraph::new(body).style(Style::default().fg(Color::White)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock as StdRwLock};
    use crate::events::StepStatus;

    fn make_test_state() -> State {
        State::new(
            PathBuf::from("/tmp/dummy.sqlite"),
            None,
            Arc::new(StdRwLock::new(None)),
        )
    }

    /// extract_tunnel_url strips suffixes correctly.
    #[test]
    fn extract_tunnel_url_stops_at_suffix() {
        assert_eq!(
            extract_tunnel_url("https://x.trycloudflare.com (probe inconclusive)").as_deref(),
            Some("https://x.trycloudflare.com")
        );
        assert_eq!(
            extract_tunnel_url("URL issued: https://x.trycloudflare.com (waiting for edge)")
                .as_deref(),
            Some("https://x.trycloudflare.com")
        );
        assert_eq!(extract_tunnel_url("no url here"), None);
        assert_eq!(extract_tunnel_url("https://"), None);
    }

    /// QR sidecar mirrors `pairing-card.tsx` so a phone scan reaches /login
    /// with `k=` pre-filled.
    #[test]
    fn render_qr_shows_app_url_and_otk_and_expiry() {
        let mut state = make_test_state();
        state.tunnel_url = Some("https://example.trycloudflare.com/".into());
        state.otk_token = Some("ABCDEF1234567890".into());
        state.otk_expires_at = Some(now_secs() + 600);

        let payload = state.qr_payload();
        assert_eq!(
            payload,
            "https://example.trycloudflare.com/login?k=ABCDEF1234567890",
            "QR payload must trim trailing slash and include the OTK"
        );

        let status = render_info::format_otk_status(&state);
        assert!(status.contains("7890"), "expected last4 in {status:?}");
        assert!(status.contains("expires in"), "expected countdown in {status:?}");

        state.otk_token = None;
        state.otk_expires_at = None;
        assert_eq!(
            state.qr_payload(),
            "https://example.trycloudflare.com/",
            "missing OTK should fall back to bare URL"
        );
    }

    /// Action line carries the live log buffer count so the user knows
    /// there's history worth pressing `l` for.
    #[test]
    fn action_line_includes_log_count() {
        assert_eq!(
            render_info::format_log_action_label(0),
            "Toggle log view (0 entries)"
        );
        assert_eq!(
            render_info::format_log_action_label(42),
            "Toggle log view (42 entries)"
        );
    }

    /// Log filter active_log_filter accessor.
    #[test]
    fn active_log_filter_returns_pattern_when_active() {
        let mut state = make_test_state();
        assert!(state.active_log_filter().is_none());
        state.log_filter = LogFilter::Active { pattern: "error".into() };
        assert_eq!(state.active_log_filter(), Some("error"));
        state.log_filter = LogFilter::Editing { input: "edit".into() };
        assert!(state.active_log_filter().is_none());
    }

    /// Confirm modal defaults to None.
    #[test]
    fn default_confirm_modal_is_none() {
        let state = make_test_state();
        assert!(matches!(state.confirm_modal, ConfirmModal::None));
    }

    /// is_ready() and tunnel_down interaction (integration smoke).
    #[test]
    fn is_ready_smoke() {
        let mut state = make_test_state();
        assert!(!state.is_ready());
        if let Some(s) = state.steps.iter_mut().find(|s| s.name == "Ready") {
            s.status = StepStatus::Done;
        }
        assert!(state.is_ready());
    }
}
