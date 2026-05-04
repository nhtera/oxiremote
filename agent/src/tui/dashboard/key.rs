// Keyboard dispatch for the TUI dashboard.
// handle_key() routes KeyEvent to the correct State mutation.
// handle_device_list_key() handles keys when DeviceList view is active.

use std::time::Duration;

use crossterm::event::KeyCode;

use crate::events::{AgentEvent, EventBus};
use crate::{approval as approval_db, one_time_keys};

use super::state::{ConfirmModal, LogFilter, State, ViewState};

/// Route a keypress when the main Dashboard view is active.
/// Returns `true` if the caller should exit the run loop (user quit).
pub(super) fn handle_dashboard_key(
    code: KeyCode,
    state: &mut State,
    event_bus: &EventBus,
) -> bool {
    // Confirm modal intercepts all keys when active.
    if state.confirm_modal != ConfirmModal::None {
        return handle_confirm_modal_key(code, state);
    }

    // Filter-edit mode intercepts most keys when active.
    if let LogFilter::Editing { .. } = &state.log_filter {
        return handle_filter_edit_key(code, state);
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            if state.help_overlay {
                state.help_overlay = false;
            } else {
                return true;
            }
        }
        KeyCode::Char('b') => return true,
        KeyCode::Char('?') => {
            state.help_overlay = !state.help_overlay;
        }
        KeyCode::Char('d') => {
            state.view_state = ViewState::DeviceList { selected: 0 };
        }
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
                            state.set_flash(format!("Approved {}", short_id(&dev.device_id)));
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
            // Clear filter when leaving log view.
            if !state.show_logs {
                state.log_filter = LogFilter::None;
            }
        }
        KeyCode::Char('f') => {
            // Toggle filter input — only meaningful in log view.
            if state.show_logs {
                state.log_filter = match &state.log_filter {
                    LogFilter::None | LogFilter::Active { .. } => {
                        LogFilter::Editing { input: String::new() }
                    }
                    LogFilter::Editing { .. } => LogFilter::None,
                };
            }
        }
        KeyCode::Char('t') => {
            // Toggle Remote Desktop service via loopback endpoint.
            let db_path = state.db_path.clone();
            let flash = toggle_desktop_service(&db_path);
            state.set_flash(flash);
        }
        KeyCode::Char('s') => {
            // Toggle autostart via loopback endpoint.
            let flash = toggle_autostart();
            state.set_flash(flash);
        }
        KeyCode::Char('R') => {
            // Rotate permanent key — show confirm modal first.
            state.confirm_modal = ConfirmModal::RotatePermanentKey;
        }
        _ => {}
    }
    false
}

/// Handle keys when a confirm modal is active.
/// Returns `true` if the run loop should exit (never — modal just dismisses).
fn handle_confirm_modal_key(code: KeyCode, state: &mut State) -> bool {
    match &state.confirm_modal {
        ConfirmModal::RotatePermanentKey => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.confirm_modal = ConfirmModal::None;
                let flash = rotate_permanent_key();
                state.set_flash(flash);
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                state.confirm_modal = ConfirmModal::None;
                state.set_flash("Key rotation cancelled");
            }
            _ => {}
        },
        ConfirmModal::None => {}
    }
    false
}

/// Handle keys while filter input is being edited in the log view.
fn handle_filter_edit_key(code: KeyCode, state: &mut State) -> bool {
    let LogFilter::Editing { input } = &mut state.log_filter else {
        return false;
    };
    match code {
        KeyCode::Enter => {
            let pattern = input.clone();
            state.log_filter = if pattern.is_empty() {
                LogFilter::None
            } else {
                LogFilter::Active { pattern }
            };
        }
        KeyCode::Esc => {
            state.log_filter = LogFilter::None;
        }
        KeyCode::Backspace => {
            let LogFilter::Editing { input } = &mut state.log_filter else {
                return false;
            };
            input.pop();
        }
        KeyCode::Char(c) => {
            let LogFilter::Editing { input } = &mut state.log_filter else {
                return false;
            };
            input.push(c);
        }
        _ => {}
    }
    false
}

/// Handle keyboard input when `ViewState::DeviceList` is active.
pub(super) fn handle_device_list_key(
    code: KeyCode,
    selected: usize,
    state: &mut State,
    event_bus: &EventBus,
) {
    use super::render_device_list::list_devices;
    match code {
        KeyCode::Char('b') | KeyCode::Esc => {
            state.view_state = ViewState::Dashboard;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let ViewState::DeviceList { selected: ref mut sel } = state.view_state {
                *sel = sel.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let devices = list_devices(&state.db_path);
            let total = devices.len();
            if let ViewState::DeviceList { selected: ref mut sel } = state.view_state
                && *sel + 1 < total
            {
                *sel += 1;
            }
        }
        KeyCode::Enter => {
            let devices = list_devices(&state.db_path);
            if let Some(dev) = devices.get(selected) {
                let device_id = dev.device_id.clone();
                let status = dev.approval_status.clone();
                match status.as_str() {
                    "pending" => {
                        match approval_db::approve_device(&state.db_path, &device_id) {
                            Ok(()) => {
                                event_bus.send(AgentEvent::DeviceApproved {
                                    device_id: device_id.clone(),
                                });
                                state.set_flash(format!("Approved {}", short_id(&device_id)));
                            }
                            Err(err) => state.set_flash(format!("Approve failed: {err}")),
                        }
                    }
                    "approved" => {
                        // Revoke approved devices via localhost endpoint.
                        let url = format!(
                            "http://localhost:8787/api/agent/devices/{}/revoke",
                            device_id
                        );
                        let did = device_id.clone();
                        std::thread::spawn(move || {
                            let _ = reqwest::blocking::Client::builder()
                                .timeout(Duration::from_secs(2))
                                .build()
                                .and_then(|c| c.post(&url).send());
                        });
                        state.set_flash(format!("Revoke requested for {}", short_id(&did)));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

// ── Loopback HTTP helpers ─────────────────────────────────────────────────────

/// Toggle Remote Desktop enabled state via POST /api/agent/services/desktop.
/// Reads current state from settings, flips it, returns flash message.
fn toggle_desktop_service(db_path: &std::path::PathBuf) -> String {
    let current = crate::settings::get_desktop_enabled(db_path);
    let new_state = !current;
    let body = format!(r#"{{"enabled":{}}}"#, new_state);
    match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .and_then(|c| {
            c.post("http://localhost:8787/api/agent/services/desktop")
                .header("Content-Type", "application/json")
                .body(body)
                .send()
        }) {
        Ok(resp) if resp.status().is_success() => {
            if new_state {
                "Remote Desktop enabled".into()
            } else {
                "Remote Desktop disabled".into()
            }
        }
        Ok(resp) => format!("RD toggle failed: HTTP {}", resp.status()),
        Err(err) => format!("RD toggle error: {err}"),
    }
}

/// Toggle autostart via GET then POST /api/agent/autostart.
fn toggle_autostart() -> String {
    // Read current state first.
    let current_enabled = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .and_then(|c| c.get("http://localhost:8787/api/agent/autostart").send())
    {
        Ok(resp) if resp.status().is_success() => resp
            .json::<serde_json::Value>()
            .ok()
            .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
            .unwrap_or(false),
        _ => false,
    };
    let new_state = !current_enabled;
    let body = format!(r#"{{"enabled":{}}}"#, new_state);
    match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .and_then(|c| {
            c.post("http://localhost:8787/api/agent/autostart")
                .header("Content-Type", "application/json")
                .body(body)
                .send()
        }) {
        Ok(resp) if resp.status().is_success() => {
            if new_state { "Autostart enabled".into() } else { "Autostart disabled".into() }
        }
        Ok(resp) => format!("Autostart toggle failed: HTTP {}", resp.status()),
        Err(err) => format!("Autostart toggle error: {err}"),
    }
}

/// Rotate the permanent API key via POST /api/agent/keys/permanent.
/// All paired device API keys derived from the previous permanent key are
/// invalidated automatically by auth::rotate_permanent_key on the server side.
fn rotate_permanent_key() -> String {
    match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .and_then(|c| c.post("http://localhost:8787/api/agent/keys/permanent").send())
    {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>() {
                Ok(v) => {
                    let last4 = v
                        .get("last4")
                        .and_then(|x| x.as_str())
                        .unwrap_or("????");
                    format!("Permanent key rotated — new key ends …{last4}. All devices must re-pair.")
                }
                Err(_) => "Permanent key rotated. All devices must re-pair.".into(),
            }
        }
        Ok(resp) => format!("Key rotation failed: HTTP {}", resp.status()),
        Err(err) => format!("Key rotation error: {err}"),
    }
}

fn copy_to_clipboard(value: &str) -> anyhow::Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(value.to_string())?;
    Ok(())
}

pub(super) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock as StdRwLock};
    use crate::tui::dashboard::state::{ConfirmModal, LogFilter, State, ViewState};

    fn make_state() -> State {
        State::new(
            std::path::PathBuf::from("/tmp/dummy.sqlite"),
            None,
            Arc::new(StdRwLock::new(None)),
        )
    }

    fn make_bus() -> Arc<EventBus> {
        EventBus::new()
    }

    #[test]
    fn quit_key_returns_true() {
        let mut state = make_state();
        let bus = make_bus();
        assert!(handle_dashboard_key(KeyCode::Char('q'), &mut state, &bus));
    }

    #[test]
    fn esc_closes_help_overlay_before_quitting() {
        let mut state = make_state();
        state.help_overlay = true;
        let bus = make_bus();
        // First Esc closes overlay, does NOT quit.
        assert!(!handle_dashboard_key(KeyCode::Esc, &mut state, &bus));
        assert!(!state.help_overlay);
        // Second Esc quits.
        assert!(handle_dashboard_key(KeyCode::Esc, &mut state, &bus));
    }

    #[test]
    fn help_toggle() {
        let mut state = make_state();
        let bus = make_bus();
        assert!(!handle_dashboard_key(KeyCode::Char('?'), &mut state, &bus));
        assert!(state.help_overlay);
        assert!(!handle_dashboard_key(KeyCode::Char('?'), &mut state, &bus));
        assert!(!state.help_overlay);
    }

    #[test]
    fn l_toggles_log_view() {
        let mut state = make_state();
        let bus = make_bus();
        assert!(!handle_dashboard_key(KeyCode::Char('l'), &mut state, &bus));
        assert!(state.show_logs);
        assert!(!handle_dashboard_key(KeyCode::Char('l'), &mut state, &bus));
        assert!(!state.show_logs);
    }

    #[test]
    fn f_opens_filter_edit_when_in_log_view() {
        let mut state = make_state();
        state.show_logs = true;
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Char('f'), &mut state, &bus);
        assert!(matches!(state.log_filter, LogFilter::Editing { .. }));
    }

    #[test]
    fn f_is_noop_when_not_in_log_view() {
        let mut state = make_state();
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Char('f'), &mut state, &bus);
        assert!(matches!(state.log_filter, LogFilter::None));
    }

    #[test]
    fn filter_edit_enter_activates_filter() {
        let mut state = make_state();
        state.show_logs = true;
        state.log_filter = LogFilter::Editing { input: "warn".into() };
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Enter, &mut state, &bus);
        assert!(matches!(&state.log_filter, LogFilter::Active { pattern } if pattern == "warn"));
    }

    #[test]
    fn filter_edit_enter_with_empty_clears_filter() {
        let mut state = make_state();
        state.log_filter = LogFilter::Editing { input: String::new() };
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Enter, &mut state, &bus);
        assert!(matches!(state.log_filter, LogFilter::None));
    }

    #[test]
    fn filter_edit_esc_cancels() {
        let mut state = make_state();
        state.log_filter = LogFilter::Editing { input: "test".into() };
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Esc, &mut state, &bus);
        assert!(matches!(state.log_filter, LogFilter::None));
    }

    #[test]
    fn filter_edit_backspace_removes_char() {
        let mut state = make_state();
        state.log_filter = LogFilter::Editing { input: "abc".into() };
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Backspace, &mut state, &bus);
        assert!(matches!(&state.log_filter, LogFilter::Editing { input } if input == "ab"));
    }

    #[test]
    fn capital_r_opens_confirm_modal() {
        let mut state = make_state();
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Char('R'), &mut state, &bus);
        assert!(matches!(state.confirm_modal, ConfirmModal::RotatePermanentKey));
    }

    #[test]
    fn confirm_modal_n_cancels() {
        let mut state = make_state();
        state.confirm_modal = ConfirmModal::RotatePermanentKey;
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Char('n'), &mut state, &bus);
        assert!(matches!(state.confirm_modal, ConfirmModal::None));
    }

    #[test]
    fn confirm_modal_esc_cancels() {
        let mut state = make_state();
        state.confirm_modal = ConfirmModal::RotatePermanentKey;
        let bus = make_bus();
        handle_dashboard_key(KeyCode::Esc, &mut state, &bus);
        assert!(matches!(state.confirm_modal, ConfirmModal::None));
    }

    #[test]
    fn device_list_b_returns_to_dashboard() {
        let mut state = make_state();
        state.view_state = ViewState::DeviceList { selected: 0 };
        let bus = make_bus();
        handle_device_list_key(KeyCode::Char('b'), 0, &mut state, &bus);
        assert!(matches!(state.view_state, ViewState::Dashboard));
    }

    #[test]
    fn device_list_esc_returns_to_dashboard() {
        let mut state = make_state();
        state.view_state = ViewState::DeviceList { selected: 0 };
        let bus = make_bus();
        handle_device_list_key(KeyCode::Esc, 0, &mut state, &bus);
        assert!(matches!(state.view_state, ViewState::Dashboard));
    }

    #[test]
    fn device_list_up_saturates_at_zero() {
        let mut state = make_state();
        state.view_state = ViewState::DeviceList { selected: 0 };
        let bus = make_bus();
        handle_device_list_key(KeyCode::Up, 0, &mut state, &bus);
        assert!(matches!(state.view_state, ViewState::DeviceList { selected: 0 }));
    }
}
