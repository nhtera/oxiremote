// State struct, constructors, and accessors for the TUI dashboard.
// Holds all mutable dashboard data; updated by event_handler.rs and key.rs.

use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::events::StepStatus;
use crate::one_time_keys;

pub(crate) const PROBE_LOG_MAX: usize = 6;
pub(crate) const LOG_HISTORY_MAX: usize = 50;
pub(crate) const FLASH_TTL: Duration = Duration::from_secs(3);

/// Which sub-view the dashboard is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ViewState {
    /// Normal dashboard (QR + info / logs).
    Dashboard,
    /// Device manager submenu with cursor index.
    DeviceList { selected: usize },
}

/// Which filter mode the log view is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LogFilter {
    None,
    /// User is editing a filter substring.
    Editing { input: String },
    /// Filter is pinned — only show entries matching the substring.
    Active { pattern: String },
}

/// Confirm modal for destructive operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmModal {
    None,
    /// Waiting for user to confirm permanent key rotation.
    RotatePermanentKey,
}

#[derive(Clone)]
pub(crate) struct ProbeEntry {
    pub(crate) attempt: u32,
    pub(crate) status: String,
    pub(crate) ok: bool,
    pub(crate) elapsed_ms: u64,
}

pub(crate) struct State {
    pub(crate) db_path: PathBuf,
    pub(crate) tunnel_url: Option<String>,
    /// Set to `Some(reason)` when `TunnelDown` fires; renders the tunnel step
    /// red and shows "tunnel down" instead of the URL.
    pub(crate) tunnel_down: Option<String>,
    pub(crate) steps: Vec<crate::tui::step_progress::Step>,
    pub(crate) connected_devices: usize,
    pub(crate) last_log: Option<String>,
    /// Last few HEAD-probe attempts; rendered as a tiny streaming console
    /// while the tunnel is up but health-check hasn't passed yet.
    pub(crate) probe_log: Vec<ProbeEntry>,
    pub(crate) otk_token: Option<String>,
    pub(crate) otk_expires_at: Option<i64>,
    /// Toggles whether the body shows the QR + info panel or the recent log
    /// stream. `l` keypress flips it.
    pub(crate) show_logs: bool,
    pub(crate) log_history: Vec<String>,
    /// Braille spinner frame index (0..=9), advanced once per redraw tick.
    pub(crate) spinner_frame: u8,
    /// Transient one-line status message — rendered in the footer for ~3 s
    /// after a hotkey action like "Copied URL" or "OTK regenerated".
    pub(crate) flash: Option<(String, SystemTime)>,
    /// Active sub-view (dashboard, device list, …).
    pub(crate) view_state: ViewState,
    /// When the Verifying step became active — used for elapsed-time display.
    pub(crate) verifying_started: Option<std::time::Instant>,
    /// True while the `?` help overlay is shown.
    pub(crate) help_overlay: bool,
    /// Cloudflare discovery worker base URL (`OXI_DISCOVERY_URL`). When set,
    /// the QR payload becomes `<discovery_url>/login?k=<temp_key>&otk=<otk>`
    /// so a phone scan reaches a standalone SPA that resolves the tunnel URL
    /// without manual entry.
    pub(crate) discovery_url: Option<String>,
    /// Latest temp key issued by the discovery worker. Shared Arc with the
    /// discovery client running on the server thread — written there, read
    /// here on every draw.
    pub(crate) discovery_temp_key: Arc<StdRwLock<Option<String>>>,
    /// Log filter state for the log view panel.
    pub(crate) log_filter: LogFilter,
    /// Confirm modal state for destructive operations.
    pub(crate) confirm_modal: ConfirmModal,
}

impl State {
    pub(crate) fn new(
        db_path: PathBuf,
        discovery_url: Option<String>,
        discovery_temp_key: Arc<StdRwLock<Option<String>>>,
    ) -> Self {
        use crate::tui::step_progress::Step;
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
            view_state: ViewState::Dashboard,
            verifying_started: None,
            help_overlay: false,
            discovery_url,
            discovery_temp_key,
            log_filter: LogFilter::None,
            confirm_modal: ConfirmModal::None,
        }
    }

    /// Mark every step up to AND including `last_done_name` as Done, clearing
    /// any stale sub-text. Used by handlers that imply a later step is current
    /// (HealthProbe → we're already past Tunneling) so a TUI that joined the
    /// bus mid-startup recovers consistent state instead of leaving earlier
    /// rows visually stuck on whatever they were initialized to.
    pub(crate) fn cascade_done_through(&mut self, last_done_name: &str) {
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

    pub(crate) fn ready_verifying(&self) -> bool {
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
    pub(crate) fn is_ready(&self) -> bool {
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
    pub(crate) fn onboarding_hint(&self) -> String {
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

    pub(crate) fn refresh_otk_from_db(&mut self) {
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

    pub(crate) fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), SystemTime::now()));
    }

    pub(crate) fn current_flash(&self) -> Option<&str> {
        self.flash.as_ref().and_then(|(msg, ts)| {
            let elapsed = SystemTime::now().duration_since(*ts).unwrap_or_default();
            if elapsed < FLASH_TTL { Some(msg.as_str()) } else { None }
        })
    }

    /// Snapshot the latest temp key written by the discovery client. Returns
    /// `None` when discovery is disabled or no key has been issued yet.
    pub(crate) fn current_discovery_temp_key(&self) -> Option<String> {
        self.discovery_temp_key
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    /// Combine tunnel URL + active OTK into the deep-link the QR encodes.
    /// In discovery mode (env `OXI_DISCOVERY_URL` set + temp key issued + OTK
    /// present) the payload becomes `<discovery_url>/login?k=<temp_key>&otk=<otk>`
    /// so a single QR scan completes pairing across origins. Otherwise mirrors
    /// `pairing-card.tsx` with the original embedded form.
    pub(crate) fn qr_payload(&self) -> String {
        if let (Some(durl), Some(tk), Some(otk)) = (
            self.discovery_url.as_deref(),
            self.current_discovery_temp_key(),
            self.otk_token.as_deref(),
        ) {
            return format!(
                "{}/login?k={}&otk={}",
                durl.trim_end_matches('/'),
                tk,
                otk
            );
        }
        match (&self.tunnel_url, &self.otk_token) {
            (Some(url), Some(otk)) => {
                format!("{}/login?k={}", url.trim_end_matches('/'), otk)
            }
            (Some(url), None) => url.clone(),
            _ => "Tunnel not ready yet…".to_string(),
        }
    }

    /// Returns the currently active log filter pattern (if any).
    pub(crate) fn active_log_filter(&self) -> Option<&str> {
        match &self.log_filter {
            LogFilter::Active { pattern } => Some(pattern.as_str()),
            _ => None,
        }
    }
}

pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock as StdRwLock};

    fn make_test_state() -> State {
        State::new(
            std::path::PathBuf::from("/tmp/dummy.sqlite"),
            None,
            Arc::new(StdRwLock::new(None)),
        )
    }

    #[test]
    fn default_state_has_no_active_steps() {
        let state = make_test_state();
        assert!(
            state.steps.iter().all(|s| !matches!(s.status, StepStatus::Active)),
            "no step should default to Active"
        );
    }

    #[test]
    fn cascade_done_through_marks_steps() {
        let mut state = make_test_state();
        state.cascade_done_through("Tunneling");
        for name in ["Preparing", "Connecting", "Tunneling"] {
            let s = state.steps.iter().find(|s| s.name == name).unwrap();
            assert!(matches!(s.status, StepStatus::Done), "{name} should be Done");
        }
        for name in ["Verifying", "Ready"] {
            let s = state.steps.iter().find(|s| s.name == name).unwrap();
            assert!(matches!(s.status, StepStatus::Pending), "{name} should be Pending");
        }
    }

    #[test]
    fn qr_payload_uses_discovery_form_when_temp_key_and_otk_present() {
        let slot = Arc::new(StdRwLock::new(Some("temp1234".to_string())));
        let mut state = State::new(
            std::path::PathBuf::from("/tmp/dummy.sqlite"),
            Some("https://oxiremote.app/".into()),
            slot,
        );
        state.tunnel_url = Some("https://example.trycloudflare.com".into());
        state.otk_token = Some("OTK_VAL".into());

        assert_eq!(
            state.qr_payload(),
            "https://oxiremote.app/login?k=temp1234&otk=OTK_VAL",
            "discovery URL must take precedence and trim trailing slash"
        );
    }

    #[test]
    fn qr_payload_falls_back_when_temp_key_missing() {
        let slot: Arc<StdRwLock<Option<String>>> = Arc::new(StdRwLock::new(None));
        let mut state = State::new(
            std::path::PathBuf::from("/tmp/dummy.sqlite"),
            Some("https://oxiremote.app".into()),
            slot,
        );
        state.tunnel_url = Some("https://example.trycloudflare.com".into());
        state.otk_token = Some("OTK_VAL".into());

        assert_eq!(
            state.qr_payload(),
            "https://example.trycloudflare.com/login?k=OTK_VAL",
            "missing temp key must fall back to embedded form, not produce bad URLs"
        );
    }

    #[test]
    fn set_flash_and_current_flash() {
        let mut state = make_test_state();
        assert!(state.current_flash().is_none());
        state.set_flash("hello");
        assert_eq!(state.current_flash(), Some("hello"));
    }

    #[test]
    fn is_ready_requires_ready_done_and_no_down() {
        let mut state = make_test_state();
        assert!(!state.is_ready());
        if let Some(s) = state.steps.iter_mut().find(|s| s.name == "Ready") {
            s.status = StepStatus::Done;
        }
        assert!(state.is_ready());
        state.tunnel_down = Some("crash".into());
        assert!(!state.is_ready());
    }
}
