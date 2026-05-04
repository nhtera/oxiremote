// AgentEvent handler for the TUI dashboard State.
// apply() is the single entry-point: called once per event drained from the bus.

use crate::events::{AgentEvent, StepStatus, TunnelStep};

use super::state::State;

impl State {
    pub(crate) fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TunnelStepChanged { step, info, reason, .. } => {
                // Late-joiner hydration: TUI subscribes to the bus only after
                // the menu, which means TunnelUrlChanged may have fired and
                // been dropped before we listen. Step events for Tunneling /
                // Verifying / Ready carry the URL in `info`. Salvage it so
                // the QR pane and Host Info don't render "—" forever.
                if self.tunnel_url.is_none()
                    && let Some(s) = info
                    && let Some(url) = super::extract_tunnel_url(s)
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

                // Record when Verifying becomes active so elapsed display works.
                if step_name == "Verifying" && self.verifying_started.is_none() {
                    self.verifying_started = Some(std::time::Instant::now());
                }
                // Once Ready, we no longer need the elapsed timer.
                if step_name == "Ready" {
                    self.verifying_started = None;
                }

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
                // Start elapsed timer for the Verifying step.
                if self.verifying_started.is_none() {
                    self.verifying_started = Some(std::time::Instant::now());
                }
            }
            AgentEvent::HealthProbe { attempt, status, ok, elapsed_ms } => {
                use super::state::{ProbeEntry, PROBE_LOG_MAX};
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
                    self.verifying_started = None;
                }
            }
            AgentEvent::DeviceConnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_add(1);
            }
            AgentEvent::DeviceDisconnected { .. } => {
                self.connected_devices = self.connected_devices.saturating_sub(1);
            }
            AgentEvent::LogEntry { msg, .. } => {
                use super::state::LOG_HISTORY_MAX;
                self.last_log = Some(msg.clone());
                self.log_history.push(msg.clone());
                if self.log_history.len() > LOG_HISTORY_MAX {
                    self.log_history.remove(0);
                }
            }
            AgentEvent::OtkIssued { .. } | AgentEvent::OtkUsed { .. } => {
                self.refresh_otk_from_db();
            }
            AgentEvent::DiscoveryTempKeyIssued { .. } => {
                // Slot has already been written by the discovery client; flash
                // is purely a "you can scan now" cue for the operator.
                self.set_flash("Discovery key issued");
            }
            AgentEvent::DiscoveryUnavailable => {
                self.set_flash("Discovery unavailable — falling back to tunnel QR");
            }
            AgentEvent::OtkExpired { .. } => {
                self.refresh_otk_from_db();
                // 5-second prominent flash — longer than the normal 3-second TTL
                // so the operator has time to read it.
                self.flash = Some((
                    "OTK expired — press r to regenerate".into(),
                    std::time::SystemTime::now(),
                ));
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
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock as StdRwLock};
    use crate::events::{AgentEvent, StepStatus, TunnelStep};
    use crate::tui::dashboard::state::State;

    fn make_test_state() -> State {
        State::new(
            std::path::PathBuf::from("/tmp/dummy.sqlite"),
            None,
            Arc::new(StdRwLock::new(None)),
        )
    }

    fn step_status(state: &State, name: &str) -> StepStatus {
        state
            .steps
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.status)
            .unwrap_or(StepStatus::Pending)
    }

    #[test]
    fn health_probe_cascades_earlier_steps_to_done() {
        let mut state = make_test_state();
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

    #[test]
    fn tunnel_url_changed_cascades_earlier_steps_to_done() {
        let mut state = make_test_state();
        state.apply(&AgentEvent::TunnelUrlChanged {
            url: "https://test.trycloudflare.com".into(),
        });
        assert!(matches!(step_status(&state, "Preparing"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Connecting"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Tunneling"), StepStatus::Done));
        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));
    }

    #[test]
    fn health_probe_ok_promotes_ready_to_done() {
        let mut state = make_test_state();
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

    #[test]
    fn tunnel_down_after_ready_flips_is_ready_false() {
        let mut state = make_test_state();
        state.apply(&AgentEvent::HealthProbe {
            attempt: 1,
            status: "200 OK".into(),
            elapsed_ms: 50,
            ok: true,
        });
        assert!(state.is_ready());

        state.apply(&AgentEvent::TunnelDown {
            reason: "exit code 1".into(),
            recovery_hint: None,
        });
        assert!(!state.is_ready(), "TunnelDown must defeat is_ready()");
        assert!(state.tunnel_down.is_some());
    }

    #[test]
    fn step_event_hydrates_tunnel_url_from_info() {
        let mut state = make_test_state();
        assert!(state.tunnel_url.is_none());

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

    #[test]
    fn snapshot_hydration_reaches_ready_without_further_events() {
        let bus = crate::events::EventBus::new();
        bus.send(AgentEvent::TunnelUrlChanged {
            url: "https://abc.trycloudflare.com".into(),
        });
        bus.send(AgentEvent::TunnelStepChanged {
            step: TunnelStep::Ready,
            attempt: 1,
            info: Some("https://abc.trycloudflare.com".into()),
            reason: None,
        });

        let snap = bus.snapshot();
        let mut state = make_test_state();
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

    #[test]
    fn failed_event_annotates_only_active_step() {
        let mut state = make_test_state();
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

        assert!(matches!(step_status(&state, "Verifying"), StepStatus::Active));
        let verifying = state.steps.iter().find(|s| s.name == "Verifying").unwrap();
        assert!(
            verifying.sub.as_deref().unwrap_or("").contains("failed:"),
            "Verifying.sub should carry 'failed: ...' annotation, got {:?}",
            verifying.sub
        );

        for name in ["Preparing", "Connecting", "Tunneling"] {
            let s = state.steps.iter().find(|s| s.name == name).unwrap();
            assert!(
                !s.sub.as_deref().unwrap_or("").contains("failed:"),
                "{name} should NOT carry a failed annotation"
            );
        }
    }
}
