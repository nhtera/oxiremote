// Main TUI dashboard. Shows the pairing QR (encoding `<tunnel>/login?k=<otk>`
// so a phone scan auto-fills the OTK), live OTK + countdown, host info, and a
// hotkey panel. Subscribes to the event bus so tunnel/device/OTK state updates
// live-refresh without polling.

pub(super) mod render_footer;
pub(super) mod render_header;
pub(super) mod render_info;
pub(super) mod render_logs;
pub(super) mod render_qr;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::approval;
use crate::events::{AgentEvent, EventBus, StepStatus, TunnelStep};
use crate::{approval as approval_db, one_time_keys};

use render_footer::{render_footer, render_onboarding_hint};
use render_header::{render_compact_header, render_header};
use render_info::render_info_panel;
use render_logs::render_logs;
use render_qr::render_qr_panel;

type Term = Terminal<CrosstermBackend<io::Stdout>>;

pub(super) struct State {
    pub(super) db_path: PathBuf,
    pub(super) tunnel_url: Option<String>,
    /// Set to `Some(reason)` when `TunnelDown` fires; renders the tunnel step
    /// red and shows "tunnel down" instead of the URL.
    pub(super) tunnel_down: Option<String>,
    pub(super) steps: Vec<super::step_progress::Step>,
    pub(super) connected_devices: usize,
    pub(super) last_log: Option<String>,
    /// Last few HEAD-probe attempts; rendered as a tiny streaming console
    /// while the tunnel is up but health-check hasn't passed yet.
    pub(super) probe_log: Vec<ProbeEntry>,
    pub(super) otk_token: Option<String>,
    pub(super) otk_expires_at: Option<i64>,
    /// Toggles whether the body shows the QR + info panel or the recent log
    /// stream. `l` keypress flips it.
    pub(super) show_logs: bool,
    pub(super) log_history: Vec<String>,
    /// Braille spinner frame index (0..=9), advanced once per redraw tick.
    pub(super) spinner_frame: u8,
    /// Transient one-line status message — rendered in the footer for ~3 s
    /// after a hotkey action like "Copied URL" or "OTK regenerated".
    pub(super) flash: Option<(String, SystemTime)>,
}

#[derive(Clone)]
pub(super) struct ProbeEntry {
    pub(super) attempt: u32,
    pub(super) status: String,
    pub(super) ok: bool,
    pub(super) elapsed_ms: u64,
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
            spinner_frame: 0,
        }
    }

    /// Mark every step up to AND including `last_done_name` as Done, clearing
    /// any stale sub-text. Used by handlers that imply a later step is current
    /// (HealthProbe → we're already past Tunneling) so a TUI that joined the
    /// bus mid-startup recovers consistent state instead of leaving earlier
    /// rows visually stuck on whatever they were initialized to.
    pub(super) fn cascade_done_through(&mut self, last_done_name: &str) {
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

    pub(super) fn ready_verifying(&self) -> bool {
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
    pub(super) fn is_ready(&self) -> bool {
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
    pub(super) fn onboarding_hint(&self) -> String {
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

    pub(super) fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), SystemTime::now()));
    }

    pub(super) fn current_flash(&self) -> Option<&str> {
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
                        let why = reason.clone().unwrap_or_else(|| "unknown error".into());
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
                    && let Some(s) = self.steps.iter_mut().find(|s| s.name == "Ready")
                {
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
            AgentEvent::OtkIssued { .. }
            | AgentEvent::OtkUsed { .. }
            | AgentEvent::OtkExpired { .. } => {
                self.refresh_otk_from_db();
            }
            AgentEvent::TunnelDown { reason, recovery_hint } => {
                // Surface the recovery hint as the tunnel-down message when
                // present; falls back to the raw `reason` so the dashboard
                // never shows an empty "tunnel down" overlay.
                self.tunnel_down = Some(match recovery_hint {
                    Some(h) => format!("{reason} — {h}"),
                    None => reason.clone(),
                });
                // Mark the currently-active tunnel step with a "down" sub-text.
                // The Tunneling step is the most appropriate anchor.
                if let Some(s) = self.steps.iter_mut().find(|s| s.name == "Tunneling") {
                    s.status = StepStatus::Active; // reuse Active coloring for dead state
                    s.sub = Some(format!(
                        "tunnel down: {}",
                        reason.chars().take(40).collect::<String>()
                    ));
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
    pub(super) fn qr_payload(&self) -> String {
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
                KeyCode::Char('r') => {
                    match one_time_keys::generate_otk(&state.db_path, None) {
                        Ok(rec) => {
                            let prefix: String = rec.token.chars().take(4).collect();
                            event_bus.send(AgentEvent::OtkIssued { token_prefix: prefix });
                            state.otk_token = Some(rec.token);
                            state.otk_expires_at = Some(rec.expires_at);
                            state.set_flash("Regenerated one-time key");
                        }
                        Err(err) => state.set_flash(format!("OTK error: {err}")),
                    }
                }
                KeyCode::Char('a') => match approval_db::list_pending(&state.db_path) {
                    Ok(devices) => match devices.first() {
                        Some(dev) => {
                            match approval_db::approve_device(&state.db_path, &dev.device_id) {
                                Ok(()) => {
                                    event_bus.send(AgentEvent::DeviceApproved {
                                        device_id: dev.device_id.clone(),
                                    });
                                    state.set_flash(format!(
                                        "Approved {}",
                                        short_id(&dev.device_id)
                                    ));
                                }
                                Err(err) => state.set_flash(format!("Approve failed: {err}")),
                            }
                        }
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

fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(value.to_string())?;
    Ok(())
}

// --- Shared helpers used by multiple render submodules ---

pub(super) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

// --- Draw dispatch ---

fn draw(f: &mut ratatui::Frame<'_>, state: &State) {
    if state.is_ready() {
        draw_dashboard(f, state);
    } else {
        draw_onboarding(f, state);
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
        // Header height = border (1) + top padding (1) + 5 rows + bottom border (1) = 8.
        // In narrow terminals (<80 cols) we fall back to the 3-row compact display at h=6.
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
            recovery_hint: None,
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

    /// Late-joiner via snapshot: simulate the TUI subscribing AFTER tunnel
    /// boot completes. Hydrating from `EventBus::snapshot()` must reach
    /// `is_ready()` without any further events arriving — otherwise the
    /// onboarding view sticks forever.
    #[test]
    fn snapshot_hydration_reaches_ready_without_further_events() {
        let bus = EventBus::new();
        // Replay the boot the TUI missed (URL fired, then Ready fired).
        bus.send(AgentEvent::TunnelUrlChanged {
            url: "https://abc.trycloudflare.com".into(),
        });
        bus.send(AgentEvent::TunnelStepChanged {
            step: TunnelStep::Ready,
            attempt: 1,
            info: Some("https://abc.trycloudflare.com".into()),
            reason: None,
        });

        // Now run the same hydration the dashboard does on entry.
        let snap = bus.snapshot();
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        if let Some(url) = snap.url {
            state.tunnel_url = Some(url);
            state.cascade_done_through("Tunneling");
        }
        if let Some(ev) = snap.latest_step {
            state.apply(&ev);
        }

        assert!(state.is_ready(), "TUI must reach Ready via snapshot alone");
        assert_eq!(
            state.tunnel_url.as_deref(),
            Some("https://abc.trycloudflare.com")
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

    /// QR sidecar mirrors `pairing-card.tsx` so a phone scan reaches /login
    /// with `k=` pre-filled. Pin: payload = `{url}/login?k={otk}` with no
    /// trailing slash on the URL even when one was passed in.
    #[test]
    fn render_qr_shows_app_url_and_otk_and_expiry() {
        let mut state = State::new(PathBuf::from("/tmp/dummy.sqlite"));
        state.tunnel_url = Some("https://example.trycloudflare.com/".into());
        state.otk_token = Some("ABCDEF1234567890".into());
        state.otk_expires_at = Some(now_secs() + 600);

        let payload = state.qr_payload();
        assert_eq!(
            payload,
            "https://example.trycloudflare.com/login?k=ABCDEF1234567890",
            "QR payload must trim trailing slash and include the OTK"
        );

        // OTK status line carries both a masked tail and the countdown so the
        // operator sees expiry without scrolling.
        let status = render_info::format_otk_status(&state);
        assert!(status.contains("7890"), "expected last4 in {status:?}");
        assert!(
            status.contains("expires in"),
            "expected countdown in {status:?}"
        );

        // No OTK → URL only, used while the operator hasn't pressed `r` yet.
        state.otk_token = None;
        state.otk_expires_at = None;
        assert_eq!(
            state.qr_payload(),
            "https://example.trycloudflare.com/",
            "missing OTK should fall back to bare URL (deep-link form requires both)"
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
}
