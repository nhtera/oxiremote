// Main TUI dashboard. Shows the pairing QR (encoding `<tunnel>/login?k=<otk>`
// so a phone scan auto-fills the OTK), live OTK + countdown, host info, and a
// hotkey panel. Subscribes to the event bus so tunnel/device/OTK state updates
// live-refresh without polling.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use qrcode::{QrCode, render::unicode};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::approval;
use crate::events::{AgentEvent, EventBus, StepStatus, TunnelStep};
use crate::{approval as approval_db, one_time_keys};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

struct State {
    db_path: PathBuf,
    tunnel_url: Option<String>,
    /// Set to `Some(reason)` when `TunnelDown` fires; renders the tunnel step
    /// red and shows "tunnel down" instead of the URL.
    tunnel_down: Option<String>,
    steps: Vec<super::step_progress::Step>,
    connected_devices: usize,
    last_log: Option<String>,
    /// Last few HEAD-probe attempts; rendered as a tiny streaming console
    /// while the tunnel is up but health-check hasn't passed yet.
    probe_log: Vec<ProbeEntry>,
    otk_token: Option<String>,
    otk_expires_at: Option<i64>,
    /// Toggles whether the body shows the QR + info panel or the recent log
    /// stream. `l` keypress flips it.
    show_logs: bool,
    log_history: Vec<String>,
    /// Transient one-line status message — rendered in the footer for ~3 s
    /// after a hotkey action like "Copied URL" or "OTK regenerated".
    flash: Option<(String, SystemTime)>,
}

#[derive(Clone)]
struct ProbeEntry {
    attempt: u32,
    status: String,
    ok: bool,
    elapsed_ms: u64,
}

const PROBE_LOG_MAX: usize = 6;
const LOG_HISTORY_MAX: usize = 50;
const FLASH_TTL: Duration = Duration::from_secs(3);

impl State {
    fn new(db_path: PathBuf) -> Self {
        use super::step_progress::Step;
        let initial_otk = one_time_keys::active_otk(&db_path).ok().flatten();
        let (otk_token, otk_expires_at) = match initial_otk {
            Some(rec) => (Some(rec.token), Some(rec.expires_at)),
            None => (None, None),
        };
        // All steps start Pending. The event bus drives transitions to
        // Active/Done. Earlier this seeded Preparing=Done and Connecting=Active
        // as a "boot looks busy" hint — but the TUI subscribes only after the
        // menu, so quick-tunnel paths emit Connecting/Tunneling before we
        // listen, and the seeded Active state for Connecting then sticks
        // forever (no later event clears it). Cascading via HealthProbe
        // (below) recovers the visual state when we miss earlier events.
        Self {
            db_path,
            tunnel_url: None,
            tunnel_down: None,
            steps: vec![
                Step { name: "Preparing".into(), status: StepStatus::Pending, sub: None },
                Step { name: "Connecting".into(), status: StepStatus::Pending, sub: None },
                Step { name: "Tunneling".into(), status: StepStatus::Pending, sub: None },
                Step { name: "Verifying".into(), status: StepStatus::Pending, sub: None },
                Step { name: "Ready".into(), status: StepStatus::Pending, sub: None },
            ],
            connected_devices: 0,
            last_log: None,
            probe_log: Vec::new(),
            otk_token,
            otk_expires_at,
            show_logs: false,
            log_history: Vec::new(),
            flash: None,
        }
    }

    /// Mark every step up to AND including `last_done_name` as Done, clearing
    /// any stale sub-text. Used by handlers that imply a later step is current
    /// (HealthProbe → we're already past Tunneling) so a TUI that joined the
    /// bus mid-startup recovers consistent state instead of leaving earlier
    /// rows visually stuck on whatever they were initialized to.
    fn cascade_done_through(&mut self, last_done_name: &str) {
        let names = ["Preparing", "Connecting", "Tunneling", "Verifying", "Ready"];
        let upto = match names.iter().position(|&n| n == last_done_name) {
            Some(i) => i,
            None => return,
        };
        for (i, s) in self.steps.iter_mut().enumerate() {
            if i <= upto {
                s.status = StepStatus::Done;
                s.sub = None;
            }
        }
    }

    fn ready_verifying(&self) -> bool {
        self.steps
            .iter()
            .find(|s| s.name == "Ready")
            .map(|s| matches!(s.status, StepStatus::Active))
            .unwrap_or(false)
    }

    /// True once the tunnel is fully serving — Ready step Done AND no down
    /// signal active. Used to gate the full 4-pane dashboard vs. the
    /// onboarding view (step checklist only). A post-Ready `TunnelDown`
    /// flips this back to false so the onboarding view re-renders with the
    /// failure surfaced inline on the checklist.
    fn is_ready(&self) -> bool {
        self.tunnel_down.is_none()
            && self
                .steps
                .iter()
                .any(|s| s.name == "Ready" && matches!(s.status, StepStatus::Done))
    }

    /// One-line summary of where the tunnel currently is — either the active
    /// step's sub-text or, if everything is Pending, a generic hint. Rendered
    /// in the onboarding footer so the operator sees live progress without
    /// the QR pane (QR is unscannable until Ready).
    fn onboarding_hint(&self) -> String {
        if let Some(reason) = &self.tunnel_down {
            return format!("tunnel down: {}", reason.chars().take(60).collect::<String>());
        }
        for s in &self.steps {
            if matches!(s.status, StepStatus::Active) {
                return match s.sub.as_ref() {
                    Some(sub) => format!("{} — {}", s.name, sub),
                    None => s.name.clone(),
                };
            }
        }
        "setting up tunnel — press q to quit, l for logs".into()
    }

    fn refresh_otk_from_db(&mut self) {
        match one_time_keys::active_otk(&self.db_path) {
            Ok(Some(rec)) => {
                self.otk_token = Some(rec.token);
                self.otk_expires_at = Some(rec.expires_at);
            }
            _ => {
                self.otk_token = None;
                self.otk_expires_at = None;
            }
        }
    }

    fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), SystemTime::now()));
    }

    fn current_flash(&self) -> Option<&str> {
        self.flash.as_ref().and_then(|(msg, ts)| {
            let elapsed = SystemTime::now().duration_since(*ts).unwrap_or_default();
            if elapsed < FLASH_TTL { Some(msg.as_str()) } else { None }
        })
    }

    fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TunnelStepChanged { step, info, reason, .. } => {
                // Late-joiner hydration: TUI subscribes to the bus only after
                // the menu, which means TunnelUrlChanged may have fired and
                // been dropped before we listen. Step events for Tunneling /
                // Verifying / Ready carry the URL in `info`. Salvage it so
                // the QR pane and Host Info don't render "—" forever.
                if self.tunnel_url.is_none()
                    && let Some(s) = info
                    && let Some(url) = extract_tunnel_url(s)
                {
                    self.tunnel_url = Some(url);
                }
                // Map TunnelStep enum → step name in the checklist.
                let (step_name, sub_text) = match step {
                    TunnelStep::Preparing => (
                        "Preparing",
                        info.clone().unwrap_or_else(|| "locating cloudflared…".into()),
                    ),
                    TunnelStep::Connecting => (
                        "Connecting",
                        info.clone().unwrap_or_else(|| "spawning cloudflared…".into()),
                    ),
                    TunnelStep::Tunneling => (
                        "Tunneling",
                        info.clone().unwrap_or_else(|| "tunnel up".into()),
                    ),
                    TunnelStep::Verifying => (
                        "Verifying",
                        info.clone().unwrap_or_else(|| "checking reachability…".into()),
                    ),
                    TunnelStep::Ready => (
                        "Ready",
                        info.clone().unwrap_or_else(|| "serving".into()),
                    ),
                    TunnelStep::Failed => {
                        // Mark the currently-active step as failed by setting sub text.
                        let why = reason
                            .clone()
                            .unwrap_or_else(|| "unknown error".into());
                        for s in &mut self.steps {
                            if matches!(s.status, StepStatus::Active) {
                                s.sub = Some(format!("failed: {why}"));
                            }
                        }
                        return;
                    }
                };

                // Mark all steps before this one as Done, this one as Active,
                // and all after as Pending. Also clear stale sub-text on
                // non-active rows so the previous step's "starting cloudflared…"
                // doesn't linger after the spinner moves on.
                let names = ["Preparing", "Connecting", "Tunneling", "Verifying", "Ready"];
                let target_idx = names.iter().position(|&n| n == step_name).unwrap_or(0);
                for (i, s) in self.steps.iter_mut().enumerate() {
                    if i < target_idx {
                        s.status = StepStatus::Done;
                        s.sub = None;
                    } else if i == target_idx {
                        // Ready is the terminal state — mark Done.
                        s.status = if step_name == "Ready" {
                            StepStatus::Done
                        } else {
                            StepStatus::Active
                        };
                        s.sub = Some(sub_text.clone());
                    } else {
                        s.status = StepStatus::Pending;
                        s.sub = None;
                    }
                }
            }
            AgentEvent::TunnelUrlChanged { url } => {
                self.tunnel_url = Some(url.clone());
                // Cascade: arriving at Verifying implies Preparing/Connecting/
                // Tunneling are done. Idempotent in the normal flow; recovers
                // visual state when those earlier events were broadcast before
                // the TUI subscribed.
                self.cascade_done_through("Tunneling");
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Tunneling") {
                    s.sub = Some(url.clone());
                }
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Verifying") {
                    s.status = StepStatus::Active;
                    s.sub = Some("verifying…".into());
                }
            }
            AgentEvent::HealthProbe { attempt, status, ok, elapsed_ms } => {
                self.probe_log.push(ProbeEntry {
                    attempt: *attempt,
                    status: status.clone(),
                    ok: *ok,
                    elapsed_ms: *elapsed_ms,
                });
                while self.probe_log.len() > PROBE_LOG_MAX {
                    self.probe_log.remove(0);
                }
                // A probe is only emitted after the tunnel transport is up —
                // mark all earlier steps Done to recover from missed events.
                self.cascade_done_through("Tunneling");
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Verifying") {
                    if *ok {
                        s.status = StepStatus::Done;
                        s.sub = Some(format!("#{attempt} ok ({}ms)", elapsed_ms));
                    } else {
                        s.status = StepStatus::Active;
                        s.sub = Some(format!("#{attempt} → {status}"));
                    }
                }
                if *ok
                    && let Some(s) = self.steps.iter_mut().find(|s| s.name == "Ready") {
                    s.status = StepStatus::Done;
                    s.sub = Some("waiting for devices".into());
                }
            }
            AgentEvent::DeviceConnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_add(1);
            }
            AgentEvent::DeviceDisconnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_sub(1);
            }
            AgentEvent::LogEntry { msg, .. } => {
                self.last_log = Some(msg.clone());
                self.log_history.push(msg.clone());
                if self.log_history.len() > LOG_HISTORY_MAX {
                    self.log_history.remove(0);
                }
            }
            AgentEvent::OtkIssued { .. } | AgentEvent::OtkUsed { .. } | AgentEvent::OtkExpired { .. } => {
                self.refresh_otk_from_db();
            }
            AgentEvent::TunnelDown { reason } => {
                self.tunnel_down = Some(reason.clone());
                // Mark the currently-active tunnel step with a "down" sub-text.
                // The Tunneling step is the most appropriate anchor.
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Tunneling") {
                    s.status = StepStatus::Active; // reuse Active coloring for dead state
                    s.sub = Some(format!("tunnel down: {}", reason.chars().take(40).collect::<String>()));
                }
                // Reset downstream steps to Pending so the checklist looks consistent.
                for name in ["Verifying", "Ready"] {
                    if let Some(s) = self.steps.iter_mut().find(|s| s.name == name) {
                        s.status = StepStatus::Pending;
                        s.sub = None;
                    }
                }
            }
            _ => {}
        }
    }

    /// Combine tunnel URL + active OTK into the deep-link the QR encodes.
    /// Mirrors `pairing-card.tsx` so a phone scan reaches /login with `k=`
    /// pre-filled and submits without keyboard typing.
    fn qr_payload(&self) -> String {
        match (&self.tunnel_url, &self.otk_token) {
            (Some(url), Some(otk)) => {
                format!("{}/login?k={}", url.trim_end_matches('/'), otk)
            }
            (Some(url), None) => url.clone(),
            _ => "Tunnel not ready yet…".to_string(),
        }
    }
}

pub fn run_dashboard(term: &mut Term, event_bus: Arc<EventBus>, db_path: PathBuf) -> Result<()> {
    let mut rx = event_bus.subscribe();
    let mut state = State::new(db_path);

    loop {
        term.draw(|f| draw(f, &state))?;

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
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('o') => {
                    if let Some(url) = &state.tunnel_url {
                        let _ = open::that(url.as_str());
                    }
                }
                KeyCode::Char('h') => {
                    let _ = open::that("http://localhost:8787/agent");
                }
                KeyCode::Char('c') => {
                    if let Some(url) = state.tunnel_url.clone() {
                        match copy_to_clipboard(&url) {
                            Ok(()) => state.set_flash("Copied tunnel URL"),
                            Err(_) => state.set_flash("Clipboard unavailable"),
                        }
                    } else {
                        state.set_flash("Tunnel not ready yet");
                    }
                }
                KeyCode::Char('k') => match state.otk_token.clone() {
                    Some(token) => match copy_to_clipboard(&token) {
                        Ok(()) => state.set_flash("Copied one-time key"),
                        Err(_) => state.set_flash("Clipboard unavailable"),
                    },
                    None => state.set_flash("No active key — press r to generate"),
                },
                KeyCode::Char('r') => match one_time_keys::generate_otk(&state.db_path, None) {
                    Ok(rec) => {
                        let prefix: String = rec.token.chars().take(4).collect();
                        event_bus.send(AgentEvent::OtkIssued { token_prefix: prefix });
                        state.otk_token = Some(rec.token);
                        state.otk_expires_at = Some(rec.expires_at);
                        state.set_flash("Regenerated one-time key");
                    }
                    Err(err) => state.set_flash(format!("OTK error: {err}")),
                },
                KeyCode::Char('a') => match approval_db::list_pending(&state.db_path) {
                    Ok(devices) => match devices.first() {
                        Some(dev) => match approval_db::approve_device(&state.db_path, &dev.device_id) {
                            Ok(()) => {
                                event_bus.send(AgentEvent::DeviceApproved {
                                    device_id: dev.device_id.clone(),
                                });
                                state.set_flash(format!("Approved {}", short_id(&dev.device_id)));
                            }
                            Err(err) => state.set_flash(format!("Approve failed: {err}")),
                        },
                        None => state.set_flash("No pending devices"),
                    },
                    Err(err) => state.set_flash(format!("List failed: {err}")),
                },
                KeyCode::Char('l') => {
                    state.show_logs = !state.show_logs;
                }
                _ => {}
            }
        }
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Pluck the first `https://...` token from free-form text. Used to recover
/// the tunnel URL from a TunnelStepChanged event's `info` field when the TUI
/// missed the original `TunnelUrlChanged` event (subscription happens after
/// the menu, so early events go to /dev/null). Stops at the first
/// whitespace, paren, or quote so suffixes like "(probe inconclusive)"
/// don't end up in the URL.
fn extract_tunnel_url(info: &str) -> Option<String> {
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

fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(value.to_string())?;
    Ok(())
}

fn draw(f: &mut ratatui::Frame<'_>, state: &State) {
    if state.is_ready() {
        draw_dashboard(f, state);
    } else {
        draw_onboarding(f, state);
    }
}

fn draw_dashboard(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    // 5-step checklist needs more vertical room than the old 3-step version.
    // Header height = border (1) + top padding (1) + 5 rows + bottom border (1) = 8.
    // In narrow terminals (<80 cols) we fall back to the 3-row compact display at h=6.
    let wide = area.width >= 80;
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

// Onboarding view: step checklist only — no QR (unscannable until Ready),
// no Host Info (most fields aren't real until Ready), no Actions list (most
// keybinds are no-ops pre-Ready). The operator sees just what matters: the
// 5-step progress and a one-line live status hint. `q` (quit) and `l`
// (toggle logs) still work via the global key handler.
fn draw_onboarding(f: &mut ratatui::Frame<'_>, state: &State) {
    let area = f.area();
    if state.show_logs {
        // Logs view stays available even pre-Ready so the operator can debug
        // a stuck startup. Keep header + log body + footer to mirror the
        // dashboard layout shape.
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
            Constraint::Min(0),            // hint area (large spacer; centers the message)
            Constraint::Length(2),         // probe-info / hint footer
            Constraint::Length(2),         // bottom spacer
        ])
        .split(area);

    render_header(f, outer[0], state, wide);
    render_onboarding_hint(f, outer[2], state);
}

fn render_onboarding_hint(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let hint = state.onboarding_hint();
    let color = if state.tunnel_down.is_some() {
        Color::Red
    } else {
        super::step_progress::BRAND
    };
    let para = Paragraph::new(Line::from(Span::styled(
        hint,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    f.render_widget(para, area);
}

fn render_header(f: &mut ratatui::Frame<'_>, area: Rect, state: &State, wide: bool) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "OxiRemote",
                Style::default()
                    .fg(super::step_progress::BRAND)
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
        super::step_progress::render_steps(f, inner, &state.steps);
    } else {
        // Narrow terminal — compact 3-row summary: Server | Tunnel | Ready.
        let compact: Vec<_> = state
            .steps
            .iter()
            .filter(|s| matches!(s.name.as_str(), "Preparing" | "Tunneling" | "Ready"))
            .collect();
        super::step_progress::render_steps_refs(f, inner, &compact);
    }
}

fn render_body(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_qr_panel(f, halves[0], state);
    render_info_panel(f, halves[1], state);
}

fn render_qr_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // Determine OTK expiry for dim styling.
    let otk_expired = match state.otk_expires_at {
        Some(exp) => now_secs() >= exp,
        None => false,
    };

    let title = if otk_expired { " Pair a device (key expired — press r) " } else { " Pair a device " };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.tunnel_url.is_none() {
        // Tunnel not yet ready — show a placeholder instead of passing the
        // fallback string into QrCode::new(), which would render garbage.
        let placeholder = Paragraph::new("Tunnel not ready yet\u{2026}")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(placeholder, inner);
        return;
    }

    let payload = state.qr_payload();
    let body = match QrCode::new(payload.as_bytes()) {
        Ok(code) => code
            .render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Light)
            .light_color(unicode::Dense1x2::Dark)
            .quiet_zone(false)
            .build(),
        Err(_) => payload,
    };

    // Dim the QR when the OTK has expired — scanning it will fail anyway.
    let qr_style = if otk_expired {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    };
    let para = Paragraph::new(body).alignment(Alignment::Center).style(qr_style);
    f.render_widget(para, inner);
}

fn render_info_panel(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Host Info ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let url = if state.tunnel_down.is_some() {
        "tunnel down — connections will fail".to_string()
    } else {
        state.tunnel_url.clone().unwrap_or_else(|| "—".to_string())
    };
    let url_style = if state.tunnel_down.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::White)
    };
    let devices_str = state.connected_devices.to_string();
    let otk_display = format_otk_status(state);
    let mut lines = vec![
        kv_styled("App URL", &url, url_style),
        kv("One-Time Key", &otk_display),
        kv("Connected Devices", &devices_str),
    ];

    if state.ready_verifying() && !state.probe_log.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Verifying tunnel…",
            Style::default()
                .fg(super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        )));
        for entry in &state.probe_log {
            let mark = if entry.ok { "✓" } else { " " };
            let color = if entry.ok { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {mark} #{:<3}", entry.attempt),
                    Style::default().fg(color)),
                Span::styled(format!("→ {} ", entry.status),
                    Style::default().fg(Color::White)),
                Span::styled(format!("({}ms)", entry.elapsed_ms),
                    Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    lines.extend(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default()
                .fg(super::step_progress::BRAND)
                .add_modifier(Modifier::BOLD),
        )),
        action_line("c", "Copy tunnel URL"),
        action_line("k", "Copy one-time key"),
        action_line("r", "Regenerate one-time key"),
        action_line("a", "Approve next pending device"),
        action_line("l", "Toggle log view"),
        action_line("o", "Open tunnel URL in browser"),
        action_line("h", "Open host dashboard (/agent)"),
        action_line("q", "Exit TUI"),
    ]);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_logs(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
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

fn format_otk_status(state: &State) -> String {
    match (&state.otk_token, state.otk_expires_at) {
        (Some(token), Some(expires_at)) => {
            let now = now_secs();
            let remaining = expires_at - now;
            let last4: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            if remaining <= 0 {
                format!("····{last4}  expired (press r)")
            } else {
                let mins = remaining / 60;
                let secs = remaining % 60;
                format!("····{last4}  expires in {mins:02}:{secs:02}")
            }
        }
        _ => "— (press r to generate)".to_string(),
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn render_footer(f: &mut ratatui::Frame<'_>, area: Rect, state: &State) {
    // Flash takes precedence so the operator sees confirmation of their last
    // hotkey press; falls back to the most recent log entry.
    let text = match (state.current_flash(), state.last_log.as_ref()) {
        (Some(flash), _) => format!(" {flash}"),
        (None, Some(msg)) => format!(" {msg}"),
        (None, None) => " OxiRemote is running. Keep this terminal open.".into(),
    };
    let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(para, area);
}

fn kv<'a>(k: &'a str, v: &'a str) -> Line<'a> {
    kv_styled(k, v, Style::default().fg(Color::White))
}

fn kv_styled<'a>(k: &'a str, v: &'a str, value_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {:<20}", k),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(v, value_style),
    ])
}

fn action_line<'a>(key: &'a str, label: &'a str) -> Line<'a> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn step_status(state: &State, name: &str) -> StepStatus {
        state
            .steps
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.status)
            .unwrap_or(StepStatus::Pending)
    }

    /// Late-joiner recovery: TUI subscribes to the bus AFTER the menu, so
    /// quick-tunnel paths emit Connecting/Tunneling before we listen. Only
    /// HealthProbe events arrive at the TUI in that case. Verify that an
    /// incoming probe event marks Preparing/Connecting/Tunneling as Done so
    /// the checklist doesn't show two simultaneous Active steps.
    #[test]
    fn health_probe_cascades_earlier_steps_to_done() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        // Default: every step Pending. Simulate the "TUI joined late" case
        // where Connecting/Tunneling events were lost.
        state.apply(&AgentEvent::HealthProbe {
            attempt: 1,
            status: "connecting…".into(),
            elapsed_ms: 4200,
            ok: false,
        });

        assert!(matches!(step_status(&state, "Preparing"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Connecting"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Tunneling"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));
        assert!(matches!(step_status(&state, "Ready"), StepStatus::Pending));
    }

    /// Same recovery via TunnelUrlChanged — the URL also implies the tunnel
    /// transport is up.
    #[test]
    fn tunnel_url_changed_cascades_earlier_steps_to_done() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        state.apply(&AgentEvent::TunnelUrlChanged {
            url: "https://test.trycloudflare.com".into(),
        });
        assert!(matches!(step_status(&state, "Preparing"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Connecting"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Tunneling"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));
    }

    /// Default state should not pre-mark any step Active — that's what caused
    /// the original "two-active-step" rendering bug when events were missed.
    #[test]
    fn default_state_has_no_active_steps() {
        let state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        assert!(
            state.steps.iter().all(|s| !matches!(s.status, StepStatus::Active)),
            "no step should default to Active"
        );
    }

    /// Successful HealthProbe must promote Ready → Done. Locks in the
    /// `if *ok && let Some(s) = ...` chain that's otherwise untested.
    #[test]
    fn health_probe_ok_promotes_ready_to_done() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        state.apply(&AgentEvent::HealthProbe {
            attempt: 5,
            status: "200 OK".into(),
            elapsed_ms: 87,
            ok: true,
        });
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Ready"), StepStatus::Done));
        assert!(state.is_ready(), "is_ready() should be true after probe ok=true");
    }

    /// `is_ready()` must flip back to false when TunnelDown fires after Ready,
    /// so the dashboard re-enters the onboarding view to surface the failure.
    #[test]
    fn tunnel_down_after_ready_flips_is_ready_false() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        // Reach Ready via successful probe.
        state.apply(&AgentEvent::HealthProbe {
            attempt: 1,
            status: "200 OK".into(),
            elapsed_ms: 50,
            ok: true,
        });
        assert!(state.is_ready());

        // Cloudflared crashes post-Ready.
        state.apply(&AgentEvent::TunnelDown {
            reason: "exit code 1".into(),
        });
        assert!(!state.is_ready(), "TunnelDown must defeat is_ready()");
        assert!(state.tunnel_down.is_some());
    }

    /// Late-joiner hydration: if the TUI missed `TunnelUrlChanged` (broadcast
    /// before the dashboard subscribed), it must still extract the tunnel
    /// URL from a step event's info field. Otherwise the QR pane and Host
    /// Info pane render "—" forever even after Ready=Done.
    #[test]
    fn step_event_hydrates_tunnel_url_from_info() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        assert!(state.tunnel_url.is_none());

        // Bare URL (matches what ensure_quick_tunnel emits for Tunneling).
        state.apply(&AgentEvent::TunnelStepChanged {
            step: TunnelStep::Tunneling,
            attempt: 1,
            info: Some("https://abc-def.trycloudflare.com".into()),
            reason: None,
        });
        assert_eq!(
            state.tunnel_url.as_deref(),
            Some("https://abc-def.trycloudflare.com")
        );
    }

    /// Ready event's info has the URL plus a soft suffix; we must stop at
    /// the first space so the suffix doesn't end up in the URL.
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

    /// A Failed event must not alter step status (StepStatus has no Failed
    /// variant) but must annotate the currently-active step's sub-text. After
    /// the cascade fix, only one step is Active at a time — so Failed touches
    /// at most one row, not two like before.
    #[test]
    fn failed_event_annotates_only_active_step() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        // Move into Verifying via cascade, simulating mid-startup failure.
        state.apply(&AgentEvent::HealthProbe {
            attempt: 12,
            status: "connecting…".into(),
            elapsed_ms: 4000,
            ok: false,
        });
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));

        state.apply(&AgentEvent::TunnelStepChanged {
            step: TunnelStep::Failed,
            attempt: 1,
            info: None,
            reason: Some("health probe timeout (180s)".into()),
        });

        // Verifying still Active (no Failed status variant), but sub-text
        // now carries the failure reason.
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));
        let verifying = state.steps.iter().find(|s| s.name == "Verifying").unwrap();
        assert!(
            verifying.sub.as_deref().unwrap_or("").contains("failed:"),
            "Verifying.sub should carry 'failed: ...' annotation, got {:?}",
            verifying.sub
        );

        // Earlier steps are Done (cascade), so they're NOT Active and must
        // not have been annotated. Catches a regression of the original
        // two-failed-step rendering bug.
        for name in ["Preparing", "Connecting", "Tunneling"] {
            let s = state.steps.iter().find(|s| s.name == name).unwrap();
            assert!(
                !s.sub.as_deref().unwrap_or("").contains("failed:"),
                "{name} should NOT carry a failed annotation"
            );
        }
    }
}
