/// WebSocket handler for remote desktop streaming.
///
/// Handles:
/// - WebRTC signaling (offer/answer/ICE) over a JSON text channel on the same WS
/// - Two DataChannels: "desktop" (binary frames, unordered) and "ctrl" (input, ordered)
/// - 5-second fallback to WS-binary when DataChannel does not open in time
/// - Input injection via `InputInjector` wrapped in `Mutex` (injector is !Sync)
/// - Quality-tier changes restart the `CaptureLoop` via a `watch` channel
///
/// Wire formats: see CLAUDE.md → "Remote desktop pipeline selection".
#[cfg(feature = "desktop")]
mod inner {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::ws::{Message, WebSocket};
    use axum::extract::{Path, Query, State, WebSocketUpgrade};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::CookieJar;
    use bytes::Bytes;
    use desktop::input::{InputInjector, MouseBtn, PressAction};
    use desktop::{InputEvent, QualityTier};
    use serde::{Deserialize, Serialize};
    use tokio::sync::{mpsc, watch, Mutex};
    use tracing::{info, warn};
    use crate::events::AgentEvent;
    use webrtc::api::APIBuilder;
    use webrtc::api::media_engine::MediaEngine;
    use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
    use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
    use webrtc::ice_transport::ice_server::RTCIceServer;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
    use webrtc::peer_connection::RTCPeerConnection;
    use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
    use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
    use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
    use webrtc::track::track_local::TrackLocal;

    use crate::auth::{
        extract_bearer_from_ws_protocols, require_owner_session_with_device_dual,
        WS_BEARER_PROTOCOL,
    };
    use crate::desktop_service::DesktopService;
    use crate::desktop_ws_capture::{spawn_capture_pipeline, Sink};
    use crate::pipeline_selection::{
        choose as choose_pipeline, operator_preference, parse_force_pipeline, ClientCapabilities,
        OperatorPref, Pipeline,
    };
    use crate::AppState;

    /// Optional per-session pipeline override, parsed from the WS upgrade
    /// query string. The SPA appends `?force_pipeline=jpeg` when honoring a
    /// `FallbackPending` so the reconnect session forces JPEG without
    /// changing the agent's env-var preference. Validated against the same
    /// allow-list as the env var; unknown values yield 400.
    ///
    /// `pub(crate)` because the route handler is `pub(crate)` and Rust
    /// requires query types to be at least as visible as the handler that
    /// names them (`private_interfaces` lint, hard error in 2024 edition).
    #[derive(Debug, Deserialize, Default)]
    pub(crate) struct DesktopWsQuery {
        #[serde(default)]
        force_pipeline: Option<String>,
    }

    // ── Wire-format types ─────────────────────────────────────────────────────

    /// Signaling messages the agent receives from the client (WS text channel).
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum SignalIn {
        Offer { sdp: String },
        Ice { candidate: RTCIceCandidateInit },
        /// Client announces decoder support. Agent uses this to pick JPEG vs
        /// H.264 pipeline. Presence of `webcodecs: true` and a codec in
        /// `codecs` matching our fmtp means H.264 is viable. `audio: true`
        /// is the SPA's opt-in for an Opus receive sink — only honoured
        /// when the operator's `desktop_audio_enabled` setting is also true
        /// AND `desktop::audio::probe_supported()` returns true. Defaults
        /// false so a missing field never silently activates audio.
        CapabilitiesClient {
            #[serde(default)]
            codecs: Vec<String>,
            #[serde(default)]
            webcodecs: bool,
            #[serde(default)]
            audio: bool,
            /// Phase-03 hardening: SPA detected loopback origin. Server
            /// disables REMB → encoder bitrate path; encoder stays at the
            /// operator's tier bitrate. See `ClientCapabilities::loopback`
            /// for the full why.
            #[serde(default)]
            loopback: bool,
        },
        /// Per-session capture settings. `hidpi=true` skips logical downscale
        /// so the encoder gets native physical pixels — costs ~4× pixel
        /// throughput on retina, paid for by a 2× bitrate multiplier in
        /// `tier_bitrate`. Toggling triggers a capture-pipeline restart
        /// because encoder dims are fixed once H.264 starts.
        Settings {
            #[serde(default)]
            hidpi: bool,
        },
        /// Pre-offer quality tier hint. Lets the encoder build at the user's
        /// chosen resolution on session-start instead of defaulting to Med
        /// then restarting on first ctrl-DC tier message. Mid-session tier
        /// changes still come via the DC `quality` WireInput.
        Quality {
            tier: String,
        },
        /// Phase-02a: client-driven audio mute mid-session. `enabled=false`
        /// fires the audio pipeline shutdown with `UserToggleOff`; video
        /// continues. `enabled=true` is logged + ignored (re-enabling needs
        /// reconnect — matches the hidpi-flip / H.264 fallback policy since
        /// the SCK audio mpsc + Opus transceiver were never created if the
        /// gate didn't pass at session-start). `enabled` is read only on
        /// `feature = "audio"` builds — dead-code allow scoped to the
        /// variant so the default build stays warning-clean.
        #[cfg_attr(not(feature = "audio"), allow(dead_code))]
        AudioToggle {
            #[serde(default)]
            enabled: bool,
        },
    }

    /// Signaling messages the agent sends to the client (WS text channel).
    #[derive(Debug, Serialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    #[allow(dead_code)] // `Pipeline` variant is only constructed with feature = "h264"
    enum SignalOut<'a> {
        Answer { sdp: &'a str },
        Ice { candidate: RTCIceCandidateInit },
        /// Sent once, before first binary frame, when 5-s DC-open timeout fires.
        Fallback,
        /// Runtime capture-output dimensions. Clients size their canvas from
        /// these values (not the logical-dimension HTTP capabilities probe);
        /// re-emitted on every tier change so the grid stays aligned.
        Capabilities {
            width: u32,
            height: u32,
            scale_factor: f32,
            tile_size: u32,
        },
        /// Phase-01: emitted once per session, immediately after the agent
        /// resolves the transport pipeline (BOTH paths). Drives the SPA's
        /// `PipelineStatusPill` so users can see why a session went JPEG vs
        /// H.264 without waiting for the avcC blob below. `hardware_accel`
        /// is `Some(false)` on JPEG and known-software builds; on H.264 it
        /// starts `None` and the subsequent `Pipeline` message (after first
        /// IDR) refines it to `Some(true)` / `Some(false)`.
        PipelineChosen {
            mode: &'a str, // "h264" | "jpeg"
            reason: &'a str,
            hardware_accel: Option<bool>,
        },
        /// Server tells client which pipeline was selected and the per-tier
        /// bitrate presets. On H.264 mode `avcc_description_b64` carries the
        /// base64-encoded `avcC` box built from the first IDR's SPS+PPS,
        /// ready for `VideoDecoder.configure({ description })`. VP9 and AV1
        /// carry their config inline (no equivalent box); `avcc_description_b64`
        /// stays `None` for those modes and on JPEG.
        ///
        /// Phase-01 additions: `reason` mirrors `PipelineChosen.reason` so a
        /// client that missed the first message can still self-describe;
        /// `hardware_accel` is authoritative once this message lands (the
        /// encoder backend is locked at that point).
        Pipeline {
            mode: &'a str, // "h264" | "vp9" | "av1" | "jpeg"
            codec: Option<&'a str>,
            tier_bitrates_kbps_low: u32,
            tier_bitrates_kbps_med: u32,
            tier_bitrates_kbps_high: u32,
            avcc_description_b64: Option<String>,
            reason: &'a str,
            hardware_accel: bool,
        },
        /// Phase-01: session-start IDR watchdog tripped. The agent has
        /// already initiated PC teardown; the client is expected to reconnect
        /// with `?force_pipeline=jpeg` so the rescue session bypasses H.264.
        /// `reason` is a stable identifier (e.g. `"no-idr-5s"`) — keep it
        /// short, it goes into the pill tooltip on the next session.
        FallbackPending {
            reason: &'a str,
        },
        /// Capture pipeline exited mid-session (permission revoked, monitor
        /// unplugged, encoder error). Client uses this to fire its reconnect
        /// modal with the reason instead of staring at a frozen frame.
        CaptureEnded {
            reason: String,
        },
        /// macOS lock-screen handling (Phase 04). Forwarded from
        /// `AgentEvent::HostLocked` into the per-session WS so the SPA can
        /// render the locked overlay without subscribing to the
        /// localhost-only `/api/agent/events` SSE.
        HostLocked { unix_ms: i64 },
        /// Mate of `HostLocked`. SPA hides the overlay and asks the active
        /// video session for an IDR so the first frame after unlock is clean.
        HostUnlocked { unix_ms: i64 },
        /// Per-session warning: agent is missing OS Accessibility permission,
        /// keyboard / mouse input may be unreliable. SPA renders an inline
        /// banner with a deep link to System Settings. Sent at most once
        /// per session, right after capability negotiation.
        AccessibilityMissing { platform: &'a str },
    }

    /// Input events the client sends on the "ctrl" DataChannel (or WS in fallback).
    /// `desktop::InputEvent` is not `Deserialize`, so we define our own wire type
    /// and convert. Uses `#[serde(tag = "t")]` as specified in the phase doc.
    #[derive(Debug, Deserialize)]
    #[serde(tag = "t", rename_all = "camelCase")]
    enum WireInput {
        Mouse {
            action: String,      // "move" | "down" | "up"
            btn: Option<String>, // "left" | "right" | "middle" — absent for "move"
            x: f32,
            y: f32,
        },
        Wheel {
            #[serde(default)]
            dx: i32,
            #[serde(default)]
            dy: i32,
        },
        Key {
            code: String,
            action: String, // "down" | "up" | "click"
            #[serde(default)]
            ctrl: bool,
            #[serde(default)]
            shift: bool,
            #[serde(default)]
            alt: bool,
            #[serde(default)]
            meta: bool,
        },
        /// Literal-text injection. Routed to `enigo.text()` server-side, which
        /// uses platform-native Unicode keystroke synthesis (no keycode lookup,
        /// no modifier flags). Server caps `s` at 4096 UTF-8 bytes.
        Text {
            s: String,
        },
        Quality {
            tier: String, // "high" | "med" | "low"
        },
        /// Mid-session HiDPI toggle. JPEG path applies live;
        /// H.264 path triggers full session restart (encoder dims locked).
        Settings {
            #[serde(default)]
            hidpi: bool,
        },
        /// Monitor selector — v1 no-op, kept for forward compatibility.
        Monitor {
            #[allow(dead_code)]
            id: u32,
        },
    }

    impl WireInput {
        /// Convert to `desktop::InputEvent`. Returns `None` for monitor events
        /// (v1 no-op) and for unrecognised field values.
        fn into_input_event(self) -> Option<InputEvent> {
            match self {
                WireInput::Mouse { action, btn, x, y } => match action.as_str() {
                    "move" => Some(InputEvent::MouseMove { x, y }),
                    "down" | "up" => {
                        let b = parse_mouse_btn(btn.as_deref()?)?;
                        let a = if action == "down" {
                            PressAction::Press
                        } else {
                            PressAction::Release
                        };
                        Some(InputEvent::MouseButton { btn: b, action: a })
                    }
                    _ => None,
                },
                WireInput::Wheel { dx, dy } => Some(InputEvent::Scroll { dx, dy }),
                WireInput::Key { code, action, ctrl, shift, alt, meta } => {
                    let a = parse_press_action(&action)?;
                    Some(InputEvent::Key { code, action: a, ctrl, shift, alt, meta })
                }
                WireInput::Text { s } => Some(InputEvent::Text { s }),
                WireInput::Quality { tier } => {
                    let t = parse_quality_tier(&tier)?;
                    Some(InputEvent::QualityChange { tier: t })
                }
                // Settings is intercepted before InputEvent conversion in
                // `dispatch_input` (it routes to `settings_tx`, not the
                // injector) — this arm is unreachable but keeps the match
                // exhaustive.
                WireInput::Settings { .. } => None,
                WireInput::Monitor { .. } => None,
            }
        }
    }

    fn parse_mouse_btn(s: &str) -> Option<MouseBtn> {
        match s {
            "left" => Some(MouseBtn::Left),
            "right" => Some(MouseBtn::Right),
            "middle" => Some(MouseBtn::Middle),
            _ => None,
        }
    }

    fn parse_press_action(s: &str) -> Option<PressAction> {
        match s {
            "down" => Some(PressAction::Press),
            "up" => Some(PressAction::Release),
            "click" => Some(PressAction::Click),
            _ => None,
        }
    }

    fn parse_quality_tier(s: &str) -> Option<QualityTier> {
        match s {
            "high" => Some(QualityTier::High),
            "med" => Some(QualityTier::Med),
            "low" => Some(QualityTier::Low),
            _ => None,
        }
    }

    // ── Axum route handler ────────────────────────────────────────────────────

    /// `GET /ws/desktop/{device_id}` — WebSocket upgrade endpoint.
    ///
    /// Returns 401 if unauthenticated, 503 if desktop is unavailable.
    pub async fn api_desktop_ws(
        ws: WebSocketUpgrade,
        Path(device_id): Path<String>,
        Query(query): Query<DesktopWsQuery>,
        State(state): State<Arc<AppState>>,
        jar: CookieJar,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        // Validate the optional `?force_pipeline=` override BEFORE doing any
        // auth work — a bad value here is a client bug, surfacing it as a
        // 400 saves a doomed WS upgrade and helps the SPA developer trace it.
        // `None` keeps the env-var preference path; `Some(_)` overrides for
        // this session only.
        let force_override: Option<OperatorPref> = match query.force_pipeline.as_deref() {
            None | Some("") => None,
            Some(v) => match parse_force_pipeline(v) {
                Some(pref) => Some(pref),
                None => return StatusCode::BAD_REQUEST.into_response(),
            },
        };
        // Browsers can't set `Authorization` on a WS upgrade, so the
        // cross-origin discovery SPA carries the api_key via subprotocols
        // (`["oxi-bearer-v1", <key>]`). Same-origin embedded mode keeps
        // using the cookie path — `bearer` is None there and dual-auth
        // falls through to the cookie session.
        let bearer = extract_bearer_from_ws_protocols(&headers);
        let Some((_session_id, session_device_id)) = require_owner_session_with_device_dual(
            &state.db_path,
            &state.signing_key,
            &jar,
            bearer.as_deref(),
        )
        .await
        else {
            return StatusCode::UNAUTHORIZED.into_response();
        };

        // The path segment must match the session's own bound device_id —
        // otherwise any authenticated user could evict another device's
        // session or impersonate its capture target.
        if session_device_id != device_id {
            return StatusCode::FORBIDDEN.into_response();
        }

        if !state.desktop_available {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }

        // Operator-controlled kill switch — toggled from the dashboard. Read
        // per-connection so flipping it off applies to the next attach without
        // touching live sessions.
        if !crate::settings::get_desktop_enabled(&state.db_path) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "remote desktop disabled by operator",
            )
                .into_response();
        }

        let Some(ref svc) = state.desktop_service else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };

        // Single-viewer: evict any previous session for this device.
        let close_rx = svc.register(&device_id);
        let svc = Arc::clone(svc);

        // Echo the marker subprotocol so the browser accepts the upgrade
        // when bearer auth was used. Browsers close the WS if the negotiated
        // subprotocol isn't in the offered list, so only set it for bearer.
        let upgrade = if bearer.is_some() {
            ws.protocols([WS_BEARER_PROTOCOL])
        } else {
            ws
        };
        upgrade.on_upgrade(move |socket| {
            desktop_session(socket, device_id, state, svc, close_rx, force_override)
        })
    }

    // ── Session entry ─────────────────────────────────────────────────────────

    async fn desktop_session(
        socket: WebSocket,
        device_id: String,
        state: Arc<AppState>,
        svc: Arc<DesktopService>,
        close_rx: tokio::sync::oneshot::Receiver<()>,
        force_override: Option<OperatorPref>,
    ) {
        info!(
            device = %device_id,
            force_pipeline = ?force_override,
            "desktop session opened"
        );
        if let Err(e) = run_session(socket, &device_id, state, close_rx, force_override).await {
            warn!(device = %device_id, error = %e, "desktop session error");
        }
        svc.remove(&device_id);
        info!(device = %device_id, "desktop session closed");
    }

    async fn run_session(
        mut socket: WebSocket,
        device_id: &str,
        state: Arc<AppState>,
        mut close_rx: tokio::sync::oneshot::Receiver<()>,
        force_override: Option<OperatorPref>,
    ) -> anyhow::Result<()> {
        // ── Quality watch channel ─────────────────────────────────────────────
        // Default to Med to match the web client's default dropdown value —
        // if the client's `quality` ctrl message is delayed we don't waste
        // cycles encoding native-retina frames no one asked for.
        #[cfg_attr(not(feature = "h264"), allow(unused_mut))]
        let (quality_tx, mut quality_rx) = watch::channel(QualityTier::Med);

        // ── Settings watch channel ────────────────────────────────────────────
        // HiDPI toggle. Initial value is filled in from the pre-offer
        // `Settings` signal (persisted in client localStorage); if the client
        // never sends one we stay in default-off.
        let (settings_tx, mut settings_rx) = watch::channel(false);
        let _ = &mut settings_rx; // silence unused-mut on JPEG-only builds

        // ── InputInjector ─────────────────────────────────────────────────────
        // Wrapped in Mutex because InputInjector is !Sync.
        let injector: Option<Arc<Mutex<InputInjector>>> = match InputInjector::new() {
            Ok(i) => Some(Arc::new(Mutex::new(i))),
            Err(e) => {
                warn!(error = %e, "InputInjector::new() failed — input disabled");
                None
            }
        };

        // Screen dimensions for normalised-coordinate → pixel mapping.
        let (screen_w, screen_h) = primary_screen_dimensions();
        // xcap's physical/logical ratio — used by the capture pipeline to
        // downscale retina frames to logical before tiling, and by us here
        // to pre-compute per-tier output dimensions for `Capabilities`.
        let scale_factor = desktop::primary_scale_factor();

        // ── Input wake ────────────────────────────────────────────────────────
        // Phase 01 idle-backoff: every ctrl-DC message bumps this Notify so
        // the JPEG capture loop breaks out of its idle sleep and ships the
        // next frame at active cadence. H.264 path doesn't use it (own
        // keyframe + RTCP PLI loop instead).
        let input_wake: desktop::capture::InputWake =
            Arc::new(tokio::sync::Notify::new());

        // ── Offer / answer exchange ───────────────────────────────────────────
        // Two-phase: read the offer + capabilities FIRST (without a PC),
        // then build the MediaEngine with the correct codec set (registering
        // pipeline is chosen), then build the PC and
        // create DataChannels.
        //
        // This ordering is required because webrtc-rs `MediaEngine::register_codec`
        // must be called BEFORE `APIBuilder::build()` + `new_peer_connection`.
        // Pre-offer ICE candidates are buffered in `await_offer_with_caps`
        // and applied to the freshly-built PC afterwards.
        //
        // Per-session pipeline override from `?force_pipeline=` is honored
        // ONLY when the operator's env preference is `Auto`. Forced env
        // (`jpeg` or `h264`) pins intent and is immutable from the client —
        // closes a privilege-escalation path where an authenticated tunnel
        // client could bypass `OXI_VIDEO_PIPELINE=jpeg` by passing
        // `?force_pipeline=h264`. Matches the plan's downgrade-only contract;
        // the watchdog-driven JPEG fallback flow still works because that
        // flow only fires when the operator was Auto in the first place
        // (the agent never picks H.264 under forced JPEG).
        let env_pref = operator_preference();
        let operator_pref = match (force_override, env_pref) {
            (Some(ov), OperatorPref::Auto) => ov,
            (Some(_), forced) => forced,
            (None, env) => env,
        };
        #[cfg_attr(not(feature = "h264"), allow(unused_variables))]
        #[cfg_attr(not(feature = "audio"), allow(unused_variables))]
        let (
            decision_pipeline,
            decision_reason,
            offer_sdp,
            initial_hidpi,
            client_audio_opt_in,
            client_loopback,
            buffered_ice,
        ) = match await_offer_with_caps(&mut socket, &mut close_rx, operator_pref, &quality_tx).await {
                Ok(v) => v,
                Err(e) => {
                    // Emit a telemetry event for the failure path so dashboards
                    // can count forced-h264-no-client and other negotiation
                    // failures. Without this the success metric "auto-h264
                    // rate" is computed against only successful sessions and
                    // hides the fail-closed denominator.
                    // `e` is an anyhow error whose Display string is the
                    // stable reason identifier (e.g. "forced-h264-no-client",
                    // "forced-av1-no-client", or "WS closed during negotiation").
                    // Extract it directly; no concrete type downcast needed.
                    let reason = e.to_string();
                    state.event_bus.send(AgentEvent::PipelineChosen {
                        device_id: device_id.to_string(),
                        pipeline: "none".to_string(),
                        reason,
                    });
                    return Err(e);
                }
            };
        #[cfg_attr(not(feature = "h264"), allow(unused_variables))]
        let pipeline = decision_pipeline;

        // ── Peer connection ───────────────────────────────────────────────────
        // Build MediaEngine with default codecs (H.264, VP9, AV1, Opus).
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let api = APIBuilder::new().with_media_engine(media_engine).build();
        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".into()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await?,
        );

        // Apply any ICE candidates that arrived before the PC existed.
        // Doing this after PC construction avoids a TOCTOU where a buffered
        // candidate arrives before ICE gathering starts.
        for ice in buffered_ice {
            if let Err(e) = pc.add_ice_candidate(ice).await {
                warn!(error = %e, "buffered pre-offer ICE candidate rejected");
            }
        }

        // ── DataChannels ──────────────────────────────────────────────────────
        // Externally negotiated: each peer pre-binds a stream id so no DCEP
        // OPEN handshake is exchanged. Asymmetry is fatal here — if the
        // server pre-binds a stream id the client never creates, the
        // browser's SCTP receives chunks on an unassigned stream and answers
        // with ERROR, collapsing the whole association ~5 s in.
        //
        // "ctrl" (id=2) is created in BOTH modes and is safe to build now —
        // the client creates it from both JPEG and H.264 hooks. "desktop"
        // (id=1) is JPEG-only and is deferred until `Pipeline::Jpeg` is
        // confirmed by `await_offer_with_caps` above.
        let ctrl_dc = pc
            .create_data_channel(
                "ctrl",
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    negotiated: Some(2),
                    ..Default::default()
                }),
            )
            .await?;

        // "cursor" (id=3): server→client cursor pose + shape sideband.
        // Decouples cursor latency from the video frame rate — the client
        // renders the sprite at network RTT instead of waiting for the
        // next captured frame (~33-100 ms on H.264 High). Created in both
        // JPEG and H.264 paths; the client always pre-binds id=3 too.
        // Ordered so shape arrives before any pose using its id.
        //
        // Currently only Windows publishes — `cursor_sideband::spawn`
        // is a no-op elsewhere. macOS keeps the SCK-composited cursor
        // in the captured frame for now (matches the current default).
        let cursor_dc = pc
            .create_data_channel(
                "cursor",
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    negotiated: Some(3),
                    ..Default::default()
                }),
            )
            .await?;
        {
            let dc = Arc::clone(&cursor_dc);
            let origin = desktop::primary_monitor_origin();
            let (w, h) = (screen_w, screen_h);
            cursor_dc.on_open(Box::new(move || {
                let dc = Arc::clone(&dc);
                Box::pin(async move {
                    crate::cursor_sideband::spawn(dc, origin, w, h);
                })
            }));
        }

        // ── Outgoing WS text channel (ICE candidates + answer) ────────────────
        // Callbacks cannot hold &mut socket, so we push through an mpsc.
        let (ws_out_tx, mut ws_out_rx) = mpsc::channel::<String>(32);

        // ── ICE callback → WS text ────────────────────────────────────────────
        let ice_ws_tx = ws_out_tx.clone();
        pc.on_ice_candidate(Box::new(move |candidate| {
            let tx = ice_ws_tx.clone();
            Box::pin(async move {
                let Some(c) = candidate else { return };
                let Ok(init) = c.to_json() else { return };
                let Ok(msg) = serde_json::to_string(&SignalOut::Ice { candidate: init })
                else {
                    return;
                };
                let _ = tx.send(msg).await;
            })
        }));

        // ── Ctrl DC: parse and dispatch input events ──────────────────────────
        // The dispatch closure also needs to re-emit `Capabilities` over the
        // WS text channel whenever the tier changes so clients resize their
        // canvas to match the new encoder output grid.
        {
            let inj = injector.clone();
            let qtx = quality_tx.clone();
            let stx = settings_tx.clone();
            let caps_ws_tx = ws_out_tx.clone();
            let wake = Arc::clone(&input_wake);
            ctrl_dc.on_message(Box::new(move |msg| {
                let inj = inj.clone();
                let qtx = qtx.clone();
                let stx = stx.clone();
                let caps_tx = caps_ws_tx.clone();
                let wake = Arc::clone(&wake);
                Box::pin(async move {
                    let text = match std::str::from_utf8(&msg.data) {
                        Ok(t) => t,
                        Err(_) => return,
                    };
                    // Wake the capture loop ahead of dispatch — every ctrl
                    // message means user activity, so even a Settings/Quality
                    // event should snap the loop back to active cadence.
                    wake.notify_one();
                    dispatch_input(
                        text,
                        &inj,
                        screen_w,
                        screen_h,
                        &qtx,
                        &stx,
                        scale_factor,
                        Some(&caps_tx),
                    )
                    .await;
                })
            }));
        }

        // Emit the immediate pill signal + telemetry event. `hardware_accel`
        // is unknown for H.264 until the encoder builds; JPEG is always SW.
        let hw_hint: Option<bool> = match pipeline {
            Pipeline::Jpeg => Some(false),
            #[cfg(feature = "h264")]
            Pipeline::H264 => None,
            // VP9 / AV1 ship software-only in the phase-01 foundation; encoder
            // backends (and any future hardware AV1 path) land in phases 02 + 05
            // / 11. Always-SW until then — hw_hint stays Some(false).
            #[cfg(feature = "vp9")]
            Pipeline::Vp9 => Some(false),
            #[cfg(feature = "av1")]
            Pipeline::Av1 => Some(false),
        };
        if let Ok(msg) = serde_json::to_string(&SignalOut::PipelineChosen {
            mode: pipeline.wire_name(),
            reason: decision_reason,
            hardware_accel: hw_hint,
        }) {
            let _ = ws_out_tx.send(msg).await;
        }
        state.event_bus.send(AgentEvent::PipelineChosen {
            device_id: device_id.to_string(),
            pipeline: pipeline.wire_name().to_string(),
            reason: decision_reason.to_string(),
        });
        // Seed the settings watch with the pre-offer client preference so
        // the encoder starts at the user's persisted HiDPI mode without a
        // reconnect round-trip.
        let _ = settings_tx.send(initial_hidpi);

        // ── macOS lock-screen handling (plan 260512-2314) ────────────────────
        //
        // 1. Stay-awake guard. Drops at end of `run_session`; that drop kills
        //    the caffeinate child + restores the screensaver idleTime. Gated
        //    behind the operator setting (default true on macOS only).
        // 2. Lock-state observer fan-out. Forwards macOS screenIsLocked /
        //    screenIsUnlocked notifications onto BOTH the global event bus
        //    (for SSE consumers like the host dashboard) AND the per-session
        //    WS (so the SPA can render its overlay without subscribing to the
        //    localhost-only SSE).
        // 3. Accessibility pre-flight: warn the SPA if input injection will
        //    be unreliable so the operator gets an actionable banner instead
        //    of "why won't my modifier keys work" frustration.
        //
        // All three are best-effort and should never block session bring-up.
        let _stay_awake_guard = if cfg!(target_os = "macos")
            && crate::settings::get_desktop_stay_awake(&state.db_path)
        {
            match desktop::macos_stay_awake::StayAwakeGuard::acquire() {
                Ok(g) => {
                    state
                        .event_bus
                        .send(AgentEvent::StayAwakeChanged { active: true });
                    Some(g)
                }
                Err(e) => {
                    warn!(error = %e, "stay-awake guard failed; session will lock normally");
                    None
                }
            }
        } else {
            None
        };
        // RAII counterpart to the StayAwakeChanged{active:true} above. Bound
        // to the same scope so the "active:false" event always fires when the
        // session ends, even on panic.
        struct StayAwakeNotifier {
            bus: Arc<crate::events::EventBus>,
            active: bool,
        }
        impl Drop for StayAwakeNotifier {
            fn drop(&mut self) {
                if self.active {
                    self.bus
                        .send(AgentEvent::StayAwakeChanged { active: false });
                }
            }
        }
        let _stay_awake_notifier = StayAwakeNotifier {
            bus: Arc::clone(&state.event_bus),
            active: _stay_awake_guard.is_some(),
        };

        // Re-emit the last known lock state once on session start so a SPA
        // joining mid-lock sees the overlay without waiting for a fresh
        // notification. macOS-only; the non-macOS shim returns None.
        if let Some(initial_lock) = desktop::macos_lock_observer::current_state() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let frame = match initial_lock {
                desktop::macos_lock_observer::LockEvent::Locked => {
                    SignalOut::HostLocked { unix_ms: now_ms }
                }
                desktop::macos_lock_observer::LockEvent::Unlocked => {
                    SignalOut::HostUnlocked { unix_ms: now_ms }
                }
            };
            if let Ok(msg) = serde_json::to_string(&frame) {
                let _ = ws_out_tx.send(msg).await;
            }
        }

        // Spawn the lock fan-out. Holds clones of ws_out_tx + event_bus and
        // forwards every observed lock/unlock until the channel closes (which
        // happens when both ws_out_tx clones are dropped at session end). The
        // task is also explicitly aborted via the AbortHandle held on the
        // stack guard below — ws_out_tx clones bleed into other tasks (the
        // ICE callback, etc.) so relying on send-error alone could leak the
        // task past session end.
        let lock_fanout_handle = {
            let lock_ws_tx = ws_out_tx.clone();
            let lock_bus = Arc::clone(&state.event_bus);
            let mut lock_rx = desktop::macos_lock_observer::subscribe();
            tokio::spawn(async move {
                while let Some(ev) = lock_rx.recv().await {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let (bus_event, frame) = match ev {
                        desktop::macos_lock_observer::LockEvent::Locked => (
                            AgentEvent::HostLocked { unix_ms: now_ms },
                            SignalOut::HostLocked { unix_ms: now_ms },
                        ),
                        desktop::macos_lock_observer::LockEvent::Unlocked => (
                            AgentEvent::HostUnlocked { unix_ms: now_ms },
                            SignalOut::HostUnlocked { unix_ms: now_ms },
                        ),
                    };
                    lock_bus.send(bus_event);
                    if let Ok(msg) = serde_json::to_string(&frame)
                        && lock_ws_tx.send(msg).await.is_err()
                    {
                        // WS receiver gone — session ended. Stop pumping.
                        break;
                    }
                }
            })
        };
        struct AbortOnDrop(tokio::task::JoinHandle<()>);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _lock_fanout_guard = AbortOnDrop(lock_fanout_handle);

        // Accessibility pre-flight: cheap (~1ms AXIsProcessTrusted call), but
        // run on a blocking thread anyway so we don't pay it in the async
        // hot path. macOS-only; non-macOS hosts have no Accessibility concept.
        #[cfg(target_os = "macos")]
        {
            let event_bus = Arc::clone(&state.event_bus);
            let acc_ws_tx = ws_out_tx.clone();
            tokio::task::spawn_blocking(move || {
                let status = desktop::permissions::PermissionStatus::check();
                if !status.accessibility {
                    warn!(
                        "session started without Accessibility permission; \
                         keyboard/mouse input will be unreliable. Grant via \
                         System Settings > Privacy & Security > Accessibility"
                    );
                    event_bus.send(AgentEvent::AccessibilityMissing { platform: "macos" });
                    if let Ok(msg) = serde_json::to_string(&SignalOut::AccessibilityMissing {
                        platform: "macos",
                    }) {
                        // Spawn a tiny task to bridge sync → async; the
                        // ws_out_tx is owned by the async runtime and we are
                        // on a blocking thread.
                        tokio::spawn(async move {
                            let _ = acc_ws_tx.send(msg).await;
                        });
                    }
                }
            });
        }
        // ─── end macOS lock-screen handling ──────────────────────────────────

        // Build the video sending track based on the chosen pipeline.
        // H.264: `TrackLocalStaticSample` with the built-in H264 payloader.
        // JPEG: no video track (DataChannel / WS binary tiles instead).
        // `sending_track` is None for JPEG; `complete_answer` skips `add_transceiver`.
        #[cfg(feature = "h264")]
        let (h264_track, sending_track): (
            Option<Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>>,
            Option<Arc<dyn TrackLocal + Send + Sync>>,
        ) = if matches!(pipeline, Pipeline::H264) {
            let t = crate::video_pipeline::new_h264_track();
            let as_trait: Arc<dyn TrackLocal + Send + Sync> = t.clone();
            (Some(t), Some(as_trait))
        } else {
            (None, None)
        };
        #[cfg(not(feature = "h264"))]
        let sending_track: Option<Arc<dyn TrackLocal + Send + Sync>> = None;

        // VP9 track — only when pipeline=Vp9 AND feature is compiled in.
        // Uses webrtc-rs's built-in VP9 RTP payloader (PT 98 reserved by
        // `register_default_codecs`), so `TrackLocalStaticSample` works the
        // same way as the H.264 path. Preserves whatever `sending_track` the
        // earlier H.264 arm set when VP9 isn't the chosen pipeline.
        #[cfg(feature = "vp9")]
        let (vp9_track, sending_track): (
            Option<Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>>,
            Option<Arc<dyn TrackLocal + Send + Sync>>,
        ) = if matches!(pipeline, Pipeline::Vp9) {
            let t = crate::vp9_pipeline::new_vp9_track();
            let as_trait: Arc<dyn TrackLocal + Send + Sync> = t.clone();
            (Some(t), Some(as_trait))
        } else {
            let existing = sending_track;
            (None, existing)
        };

        // AV1 track — same pattern as VP9: webrtc-rs ships a built-in AV1
        // RTP payloader (PT 41 reserved by `register_default_codecs`) so
        // `TrackLocalStaticSample` carries OBU fragmentation for us.
        #[cfg(feature = "av1")]
        let (av1_track, sending_track): (
            Option<Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>>,
            Option<Arc<dyn TrackLocal + Send + Sync>>,
        ) = if matches!(pipeline, Pipeline::Av1) {
            let t = crate::av1_pipeline::new_av1_track();
            let as_trait: Arc<dyn TrackLocal + Send + Sync> = t.clone();
            (Some(t), Some(as_trait))
        } else {
            let existing = sending_track;
            (None, existing)
        };

        // Audio infrastructure gate. Three operator-level sources of truth
        // must agree before we add the Opus transceiver: pipeline is a
        // video-RTP one (H.264/VP9/AV1), the DB setting
        // `desktop_audio_enabled` is on, and the build-side probe
        // (`desktop::audio::probe_supported`) succeeds. Privacy posture:
        // default-OFF wins on any disagreement, no silent activation.
        //
        // `client_audio_opt_in` is intentionally NOT part of this gate any
        // more — the client toggle is a per-listener mute, not an
        // infrastructure decision. When the client opens muted we still
        // negotiate the transceiver + run the pipeline, then drop frames at
        // the writer via `audio_muted`. That makes "Enable audio" instant
        // (flip the atomic) instead of requiring a session reconnect to
        // re-add a previously-absent transceiver.
        //
        // JPEG sessions ride a DataChannel for tiles and have no PC to
        // attach audio to. The codec-arm `matches!` blocks resolve to
        // `false` when the feature isn't compiled, so the gate only opens
        // for codecs the binary actually supports.
        #[cfg(feature = "audio")]
        let pipeline_supports_audio: bool = {
            #[allow(unused_mut)]
            let mut ok = false;
            #[cfg(feature = "h264")]
            { ok = ok || matches!(pipeline, Pipeline::H264); }
            #[cfg(feature = "vp9")]
            { ok = ok || matches!(pipeline, Pipeline::Vp9); }
            #[cfg(feature = "av1")]
            { ok = ok || matches!(pipeline, Pipeline::Av1); }
            ok
        };
        #[cfg(feature = "audio")]
        let (opus_track, audio_sending_track): (
            Option<Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>>,
            Option<Arc<dyn TrackLocal + Send + Sync>>,
        ) = if pipeline_supports_audio
            && crate::settings::get_desktop_audio_enabled(&state.db_path)
            && desktop::audio::probe_supported()
        {
            let t = crate::desktop_audio_pipeline::new_opus_track();
            let as_trait: Arc<dyn TrackLocal + Send + Sync> = t.clone();
            info!(
                pipeline = ?pipeline,
                initial_muted = !client_audio_opt_in,
                "audio gate passed: opus transceiver will be added before SRD"
            );
            (Some(t), Some(as_trait))
        } else {
            tracing::debug!(
                client_audio = client_audio_opt_in,
                "audio gate did not pass (any of: pipeline is JPEG, setting off, probe failed)"
            );
            (None, None)
        };
        // User-level mute. Initialised from the client's session-start
        // opt-in: if the client opened with audio toggled off, we start
        // muted (transceiver still present, just dropping samples at the
        // writer). Mid-session `audioToggle` flips this atomic — both
        // directions, no teardown, no renegotiation.
        #[cfg(feature = "audio")]
        let audio_muted: Arc<std::sync::atomic::AtomicBool> = Arc::new(
            std::sync::atomic::AtomicBool::new(!client_audio_opt_in),
        );
        #[cfg(not(feature = "audio"))]
        let audio_sending_track: Option<Arc<dyn TrackLocal + Send + Sync>> = None;
        // Silence the unused-binding warning when feature="audio" is on but
        // we don't yet pass `opus_track` to spawn_audio_pipeline (step 6e).
        #[cfg(feature = "audio")]
        let _ = &opus_track;

        let video_sender =
            complete_answer(&pc, offer_sdp, &ws_out_tx, sending_track, audio_sending_track)
                .await?;
        let _ = &video_sender; // silence unused when neither h264 nor vp9 nor av1 is on

        // Branch the transport pipeline now that the answer is on the wire.
        // H.264 uses an RTP track — no DC-open race, no JPEG tile encoder.
        #[cfg(feature = "h264")]
        if let (Pipeline::H264, Some(track), Some(sender)) =
            (pipeline, h264_track.as_ref(), video_sender.as_ref())
        {
            let result = run_h264_session(
                &mut socket,
                &pc,
                &ws_out_tx,
                &mut ws_out_rx,
                Arc::clone(track),
                Arc::clone(sender),
                &injector,
                screen_w,
                screen_h,
                scale_factor,
                initial_hidpi,
                client_loopback,
                &quality_tx,
                &mut quality_rx,
                &settings_tx,
                &mut settings_rx,
                &mut close_rx,
                decision_reason,
                device_id,
                &state,
                #[cfg(feature = "audio")]
                opus_track.clone(),
                #[cfg(feature = "audio")]
                Arc::clone(&audio_muted),
            )
            .await;
            let _ = pc.close().await;
            return result;
        }

        // VP9 dispatch — uses `TrackLocalStaticSample` (no custom payloader;
        // webrtc-rs has built-in VP9 RTP at PT 98) and `Vp9SequenceInfo` (no
        // out-of-band config blob — VP9 keyframes carry the sequence header
        // inline). Active-map (dirty-rect block skip) lands in phase-03b.
        #[cfg(feature = "vp9")]
        if let (Pipeline::Vp9, Some(track)) = (pipeline, vp9_track.as_ref()) {
            use desktop::capture::CaptureLoop;
            use desktop::encoders::BitrateBps;
            use desktop::resize_dims;

            let initial_tier = *quality_rx.borrow();
            let hidpi = initial_hidpi;
            let (out_w, out_h) =
                resize_dims(screen_w, screen_h, initial_tier, scale_factor, hidpi);

            if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                width: out_w,
                height: out_h,
                scale_factor,
                tile_size: desktop::TILE_SIZE,
            }) {
                let _ = ws_out_tx.send(msg).await;
            }

            let Some(vp9_sender) = video_sender else {
                warn!("vp9: complete_answer returned no sender — RTCP feedback unavailable");
                let _ = pc.close().await;
                return Ok(());
            };

            // Channels for pipeline + RTCP reader lifecycle.
            let (bgra_tx, bgra_rx) = mpsc::channel::<desktop::RawBgraFrame>(2);
            let (bitrate_tx, bitrate_rx) =
                watch::channel(tier_bitrate_vp9(initial_tier, scale_factor, hidpi));
            let (pli_tx, pli_rx) = mpsc::channel::<()>(4);
            let (seq_info_tx, seq_info_rx) =
                tokio::sync::oneshot::channel::<desktop::encoders::Vp9SequenceInfo>();
            let (vp_shutdown_tx, vp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (rtcp_shutdown_tx, rtcp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (cap_iframe_tx, cap_iframe_rx) = tokio::sync::oneshot::channel::<()>();
            let _ = cap_iframe_tx.send(()); // force first frame to be IDR

            let (abr_tx, _abr_rx_dropped) = crate::desktop_abr::channel();

            // Audio channel pair + shutdown for the VP9 session.
            #[cfg(feature = "audio")]
            let (vp9_audio_tx, vp9_audio_rx_opt) = if opus_track.is_some() {
                let (tx, rx) = mpsc::channel::<Vec<i16>>(8);
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };
            #[cfg(feature = "audio")]
            let (vp9_ap_shutdown_tx, vp9_ap_shutdown_rx) = tokio::sync::oneshot::channel::<
                crate::desktop_audio_pipeline::StopReason,
            >();
            #[cfg(feature = "audio")]
            let mut vp9_ap_shutdown_tx = Some(vp9_ap_shutdown_tx);

            // Capture: BGRA frames → bgra_tx (and PCM → vp9_audio_tx when
            // audio is gated on).
            let cap_fps_rx = quality_tx.subscribe();
            #[cfg(feature = "audio")]
            let cap_audio_tx = vp9_audio_tx;
            #[cfg(not(feature = "audio"))]
            let cap_audio_tx: Option<mpsc::Sender<Vec<i16>>> = None;
            tokio::task::spawn_blocking(move || {
                CaptureLoop::run_bgra(
                    initial_tier,
                    cap_fps_rx,
                    bgra_tx,
                    scale_factor,
                    hidpi,
                    Some(cap_iframe_rx),
                    cap_audio_tx,
                    None, // diag counters not plumbed for vp9 phase-03
                )
            });

            #[cfg(feature = "audio")]
            if let Some(track) = opus_track.as_ref() {
                try_spawn_session_audio_pipeline(
                    track,
                    vp9_audio_rx_opt,
                    vp9_ap_shutdown_rx,
                    &mut vp9_ap_shutdown_tx,
                    Arc::clone(&audio_muted),
                    &state,
                    device_id,
                );
            } else {
                let _ = vp9_audio_rx_opt;
                drop(vp9_ap_shutdown_rx);
            }

            crate::vp9_pipeline::spawn_vp9_pipeline(
                crate::vp9_pipeline::Vp9PipelineConfig {
                    width: out_w,
                    height: out_h,
                    initial_bitrate: tier_bitrate_vp9(initial_tier, scale_factor, hidpi),
                    track: Arc::clone(track),
                    bgra_rx,
                    bitrate_rx,
                    fps_rx: quality_tx.subscribe(),
                    shutdown_rx: vp_shutdown_rx,
                    pli_rx,
                    seq_info_tx,
                    frames_encoded_ok: None,
                    frames_encoded_err: None,
                    abr_tx: abr_tx.clone(),
                },
            );

            crate::video_pipeline::spawn_rtcp_reader(
                vp9_sender,
                pli_tx,
                bitrate_tx.clone(),
                abr_tx.clone(),
                rtcp_shutdown_rx,
                client_loopback,
            );

            if let Some(svc) = state.desktop_service.as_ref() {
                svc.attach_abr_tx(device_id, abr_tx.clone(), "vp9");
            }

            // P1: ABR controller for VP9 sessions. Same shape as the H.264
            // path — REMB observations drive `bitrate_tx`, which the encoder
            // consumes via its `bitrate_rx` watch. Loopback origins skip the
            // controller (no signal to act on; encoder rides operator tier).
            let (vp9_abr_shutdown_tx, vp9_abr_shutdown_rx) =
                tokio::sync::oneshot::channel::<()>();
            let mut vp9_abr_shutdown_tx = Some(vp9_abr_shutdown_tx);
            if !client_loopback {
                let initial_kbps = tier_bitrate_vp9(initial_tier, scale_factor, hidpi).0 / 1_000;
                let floor_kbps = BitrateBps::LOW.0 / 1_000;
                crate::desktop_abr::spawn_controller(crate::desktop_abr::ControllerConfig {
                    abr_rx: abr_tx.subscribe(),
                    bitrate_tx: bitrate_tx.clone(),
                    event_bus: Arc::clone(&state.event_bus),
                    device_id: device_id.to_string(),
                    floor_kbps,
                    ceiling_kbps: initial_kbps,
                    tunables: crate::desktop_abr::AbrTunables::from_env(),
                    shutdown_rx: vp9_abr_shutdown_rx,
                });
            } else {
                drop(vp9_abr_shutdown_rx);
                vp9_abr_shutdown_tx = None;
                info!(device = %device_id, "vp9: loopback origin — ABR controller skipped");
            }

            info!(
                width = out_w,
                height = out_h,
                tier = ?initial_tier,
                hidpi,
                "vp9 pipeline spawned"
            );

            // Session-start IDR watchdog: same 5-second budget as H.264.
            let mut seq_info_rx = Some(seq_info_rx);
            let connected_notify = Arc::new(tokio::sync::Notify::new());
            {
                let notify = Arc::clone(&connected_notify);
                let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
                pc.on_peer_connection_state_change(Box::new(move |state| {
                    let notify = Arc::clone(&notify);
                    let fired = Arc::clone(&fired);
                    Box::pin(async move {
                        use std::sync::atomic::Ordering;
                        use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
                        if matches!(state, RTCPeerConnectionState::Connected)
                            && !fired.swap(true, Ordering::SeqCst)
                        {
                            notify.notify_waiters();
                        }
                    })
                }));
            }
            let watchdog_notify = Arc::clone(&connected_notify);
            let watchdog = async move {
                watchdog_notify.notified().await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };
            tokio::pin!(watchdog);

            let current_hidpi = hidpi;
            loop {
                tokio::select! {
                    biased;

                    _ = &mut close_rx => break,

                    // First IDR — emit pipeline signal with hw_accel flag.
                    res = wait_vp9_seq_info(seq_info_rx.as_mut()), if seq_info_rx.is_some() => {
                        seq_info_rx = None;
                        if let Some(info) = res {
                            // Phase-08: surface the VP9 codec-specific tier
                            // bitrates so the SPA stats overlay reflects
                            // the real targets (not H.264's presets).
                            let low = tier_bitrate_vp9(QualityTier::Low, 1.0, false).0 / 1_000;
                            let med = tier_bitrate_vp9(QualityTier::Med, 1.0, false).0 / 1_000;
                            let high = tier_bitrate_vp9(QualityTier::High, 1.0, false).0 / 1_000;
                            let _ = BitrateBps::LOW; // silence unused-import
                            let msg = SignalOut::Pipeline {
                                mode: "vp9",
                                codec: Some("vp9-profile0"),
                                tier_bitrates_kbps_low: low,
                                tier_bitrates_kbps_med: med,
                                tier_bitrates_kbps_high: high,
                                avcc_description_b64: None,
                                reason: decision_reason,
                                hardware_accel: info.is_hardware,
                            };
                            if let Ok(txt) = serde_json::to_string(&msg) {
                                let _ = ws_out_tx.send(txt).await;
                            }
                            info!(hardware_accel = info.is_hardware, "vp9: pipeline signal sent");
                        }
                    }

                    _ = &mut watchdog, if seq_info_rx.is_some() => {
                        warn!(
                            device = %device_id,
                            "vp9: session-start IDR watchdog tripped — signaling FallbackPending"
                        );
                        if let Ok(txt) = serde_json::to_string(&SignalOut::FallbackPending {
                            reason: "no-idr-5s",
                        }) {
                            let _ = socket.send(Message::Text(txt.into())).await;
                        }
                        state.event_bus.send(AgentEvent::PipelineChosen {
                            device_id: device_id.to_string(),
                            pipeline: "jpeg".to_string(),
                            reason: "fallback-idr-watchdog".to_string(),
                        });
                        break;
                    }

                    Some(text) = ws_out_rx.recv() => {
                        if socket.send(Message::Text(text.into())).await.is_err() { break; }
                    }

                    Ok(_) = quality_rx.changed() => {
                        let new_tier = *quality_rx.borrow();
                        let (w, h) = resize_dims(
                            screen_w, screen_h, new_tier, scale_factor, current_hidpi,
                        );
                        // Encoder dims are locked at init. If the new tier
                        // resizes capture, break out so the SPA reconnect
                        // rebuilds the session at the new dims (same path
                        // hidpi-flip uses). Otherwise just push bitrate live.
                        if (w, h) != (out_w, out_h) {
                            info!(old = ?(out_w, out_h), new = ?(w, h), new_tier = ?new_tier,
                                "vp9: tier change resizes capture → session restart");
                            break;
                        }
                        let _ = bitrate_tx.send(tier_bitrate_vp9(new_tier, scale_factor, current_hidpi));
                        if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                            width: w, height: h, scale_factor, tile_size: desktop::TILE_SIZE,
                        }) {
                            let _ = ws_out_tx.send(msg).await;
                        }
                    }

                    Ok(_) = settings_rx.changed() => {
                        let new_hidpi = *settings_rx.borrow();
                        if new_hidpi != current_hidpi {
                            info!(old = current_hidpi, new = new_hidpi,
                                "vp9: hidpi change → session restart");
                            break;
                        }
                    }

                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                #[cfg(feature = "audio")]
                                if text.contains("audioToggle")
                                    && let Ok(SignalIn::AudioToggle { enabled }) =
                                        serde_json::from_str::<SignalIn>(&text)
                                {
                                    handle_client_audio_toggle(
                                        enabled,
                                        &audio_muted,
                                        opus_track.is_some(),
                                        device_id,
                                    );
                                    continue;
                                }
                                on_incoming_text(
                                    &text,
                                    &pc,
                                    &injector,
                                    screen_w,
                                    screen_h,
                                    &quality_tx,
                                    &settings_tx,
                                    scale_factor,
                                    &ws_out_tx,
                                    None,
                                ).await;
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                }
            }

            let _ = vp_shutdown_tx.send(());
            let _ = rtcp_shutdown_tx.send(());
            if let Some(tx) = vp9_abr_shutdown_tx.take() {
                let _ = tx.send(());
            }
            #[cfg(feature = "audio")]
            if let Some(tx) = vp9_ap_shutdown_tx.take() {
                let _ = tx.send(crate::desktop_audio_pipeline::StopReason::SessionClosed);
            }
            let _ = pc.close().await;
            return Ok(());
        }

        // AV1 dispatch — same shape as VP9; uses libaom encoder with
        // AOM_CONTENT_SCREEN (palette + IBC matching Chrome Remote Desktop).
        // webrtc-rs's built-in AV1 RTP payloader (PT 41) handles OBU
        // fragmentation per RFC 9606 via TrackLocalStaticSample.
        #[cfg(feature = "av1")]
        if let (Pipeline::Av1, Some(track)) = (pipeline, av1_track.as_ref()) {
            use desktop::capture::CaptureLoop;
            use desktop::encoders::BitrateBps;
            use desktop::resize_dims;

            let initial_tier = *quality_rx.borrow();
            let hidpi = initial_hidpi;
            let (out_w, out_h) =
                resize_dims(screen_w, screen_h, initial_tier, scale_factor, hidpi);

            if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                width: out_w,
                height: out_h,
                scale_factor,
                tile_size: desktop::TILE_SIZE,
            }) {
                let _ = ws_out_tx.send(msg).await;
            }

            let Some(av1_sender) = video_sender else {
                warn!("av1: complete_answer returned no sender — RTCP feedback unavailable");
                let _ = pc.close().await;
                return Ok(());
            };

            let (bgra_tx, bgra_rx) = mpsc::channel::<desktop::RawBgraFrame>(2);
            let (bitrate_tx, bitrate_rx) =
                watch::channel(tier_bitrate_av1(initial_tier, scale_factor, hidpi));
            let (pli_tx, pli_rx) = mpsc::channel::<()>(4);
            let (seq_info_tx, seq_info_rx) =
                tokio::sync::oneshot::channel::<desktop::encoders::Av1SequenceInfo>();
            let (vp_shutdown_tx, vp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (rtcp_shutdown_tx, rtcp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (cap_iframe_tx, cap_iframe_rx) = tokio::sync::oneshot::channel::<()>();
            let _ = cap_iframe_tx.send(());

            let (abr_tx, _abr_rx_dropped) = crate::desktop_abr::channel();

            // Audio channel pair + shutdown for the AV1 session.
            #[cfg(feature = "audio")]
            let (av1_audio_tx, av1_audio_rx_opt) = if opus_track.is_some() {
                let (tx, rx) = mpsc::channel::<Vec<i16>>(8);
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };
            #[cfg(feature = "audio")]
            let (av1_ap_shutdown_tx, av1_ap_shutdown_rx) = tokio::sync::oneshot::channel::<
                crate::desktop_audio_pipeline::StopReason,
            >();
            #[cfg(feature = "audio")]
            let mut av1_ap_shutdown_tx = Some(av1_ap_shutdown_tx);

            let cap_fps_rx = quality_tx.subscribe();
            #[cfg(feature = "audio")]
            let cap_audio_tx = av1_audio_tx;
            #[cfg(not(feature = "audio"))]
            let cap_audio_tx: Option<mpsc::Sender<Vec<i16>>> = None;
            tokio::task::spawn_blocking(move || {
                CaptureLoop::run_bgra(
                    initial_tier,
                    cap_fps_rx,
                    bgra_tx,
                    scale_factor,
                    hidpi,
                    Some(cap_iframe_rx),
                    cap_audio_tx,
                    None,
                )
            });

            #[cfg(feature = "audio")]
            if let Some(track) = opus_track.as_ref() {
                try_spawn_session_audio_pipeline(
                    track,
                    av1_audio_rx_opt,
                    av1_ap_shutdown_rx,
                    &mut av1_ap_shutdown_tx,
                    Arc::clone(&audio_muted),
                    &state,
                    device_id,
                );
            } else {
                let _ = av1_audio_rx_opt;
                drop(av1_ap_shutdown_rx);
            }

            crate::av1_pipeline::spawn_av1_pipeline(
                crate::av1_pipeline::Av1PipelineConfig {
                    width: out_w,
                    height: out_h,
                    initial_bitrate: tier_bitrate_av1(initial_tier, scale_factor, hidpi),
                    track: Arc::clone(track),
                    bgra_rx,
                    bitrate_rx,
                    fps_rx: quality_tx.subscribe(),
                    shutdown_rx: vp_shutdown_rx,
                    pli_rx,
                    seq_info_tx,
                    frames_encoded_ok: None,
                    frames_encoded_err: None,
                    abr_tx: abr_tx.clone(),
                },
            );

            crate::video_pipeline::spawn_rtcp_reader(
                av1_sender,
                pli_tx,
                bitrate_tx.clone(),
                abr_tx.clone(),
                rtcp_shutdown_rx,
                client_loopback,
            );

            if let Some(svc) = state.desktop_service.as_ref() {
                svc.attach_abr_tx(device_id, abr_tx.clone(), "av1");
            }

            // P1: ABR controller for AV1 (mirrors VP9). Loopback origins
            // skip the controller since GCC collapses on same-machine paths.
            let (av1_abr_shutdown_tx, av1_abr_shutdown_rx) =
                tokio::sync::oneshot::channel::<()>();
            let mut av1_abr_shutdown_tx = Some(av1_abr_shutdown_tx);
            if !client_loopback {
                let initial_kbps = tier_bitrate_av1(initial_tier, scale_factor, hidpi).0 / 1_000;
                let floor_kbps = BitrateBps::LOW.0 / 1_000;
                crate::desktop_abr::spawn_controller(crate::desktop_abr::ControllerConfig {
                    abr_rx: abr_tx.subscribe(),
                    bitrate_tx: bitrate_tx.clone(),
                    event_bus: Arc::clone(&state.event_bus),
                    device_id: device_id.to_string(),
                    floor_kbps,
                    ceiling_kbps: initial_kbps,
                    tunables: crate::desktop_abr::AbrTunables::from_env(),
                    shutdown_rx: av1_abr_shutdown_rx,
                });
            } else {
                drop(av1_abr_shutdown_rx);
                av1_abr_shutdown_tx = None;
                info!(device = %device_id, "av1: loopback origin — ABR controller skipped");
            }

            info!(
                width = out_w,
                height = out_h,
                tier = ?initial_tier,
                hidpi,
                "av1 pipeline spawned"
            );

            let mut seq_info_rx = Some(seq_info_rx);
            let connected_notify = Arc::new(tokio::sync::Notify::new());
            {
                let notify = Arc::clone(&connected_notify);
                let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
                pc.on_peer_connection_state_change(Box::new(move |state| {
                    let notify = Arc::clone(&notify);
                    let fired = Arc::clone(&fired);
                    Box::pin(async move {
                        use std::sync::atomic::Ordering;
                        use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
                        if matches!(state, RTCPeerConnectionState::Connected)
                            && !fired.swap(true, Ordering::SeqCst)
                        {
                            notify.notify_waiters();
                        }
                    })
                }));
            }
            let watchdog_notify = Arc::clone(&connected_notify);
            let watchdog = async move {
                watchdog_notify.notified().await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };
            tokio::pin!(watchdog);

            let current_hidpi = hidpi;
            loop {
                tokio::select! {
                    biased;

                    _ = &mut close_rx => break,

                    res = wait_av1_seq_info(seq_info_rx.as_mut()), if seq_info_rx.is_some() => {
                        seq_info_rx = None;
                        if let Some(info) = res {
                            // Phase-08: surface AV1's codec-specific tier
                            // bitrates (lower than VP9 — AV1 + screen tools
                            // hit equivalent quality at ~70% of VP9).
                            let low = tier_bitrate_av1(QualityTier::Low, 1.0, false).0 / 1_000;
                            let med = tier_bitrate_av1(QualityTier::Med, 1.0, false).0 / 1_000;
                            let high = tier_bitrate_av1(QualityTier::High, 1.0, false).0 / 1_000;
                            let _ = BitrateBps::LOW;
                            let msg = SignalOut::Pipeline {
                                mode: "av1",
                                codec: Some("av1-main"),
                                tier_bitrates_kbps_low: low,
                                tier_bitrates_kbps_med: med,
                                tier_bitrates_kbps_high: high,
                                avcc_description_b64: None,
                                reason: decision_reason,
                                hardware_accel: info.is_hardware,
                            };
                            if let Ok(txt) = serde_json::to_string(&msg) {
                                let _ = ws_out_tx.send(txt).await;
                            }
                            info!(hardware_accel = info.is_hardware, "av1: pipeline signal sent");
                        }
                    }

                    _ = &mut watchdog, if seq_info_rx.is_some() => {
                        warn!(
                            device = %device_id,
                            "av1: session-start IDR watchdog tripped — signaling FallbackPending"
                        );
                        if let Ok(txt) = serde_json::to_string(&SignalOut::FallbackPending {
                            reason: "no-idr-5s",
                        }) {
                            let _ = socket.send(Message::Text(txt.into())).await;
                        }
                        state.event_bus.send(AgentEvent::PipelineChosen {
                            device_id: device_id.to_string(),
                            pipeline: "jpeg".to_string(),
                            reason: "fallback-idr-watchdog".to_string(),
                        });
                        break;
                    }

                    Some(text) = ws_out_rx.recv() => {
                        if socket.send(Message::Text(text.into())).await.is_err() { break; }
                    }

                    Ok(_) = quality_rx.changed() => {
                        let new_tier = *quality_rx.borrow();
                        let (w, h) = resize_dims(
                            screen_w, screen_h, new_tier, scale_factor, current_hidpi,
                        );
                        // Encoder dims are locked at init — break on resize.
                        // See vp9 dispatcher for full rationale.
                        if (w, h) != (out_w, out_h) {
                            info!(old = ?(out_w, out_h), new = ?(w, h), new_tier = ?new_tier,
                                "av1: tier change resizes capture → session restart");
                            break;
                        }
                        let _ = bitrate_tx.send(tier_bitrate_av1(new_tier, scale_factor, current_hidpi));
                        if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                            width: w, height: h, scale_factor, tile_size: desktop::TILE_SIZE,
                        }) {
                            let _ = ws_out_tx.send(msg).await;
                        }
                    }

                    Ok(_) = settings_rx.changed() => {
                        let new_hidpi = *settings_rx.borrow();
                        if new_hidpi != current_hidpi {
                            info!(old = current_hidpi, new = new_hidpi,
                                "av1: hidpi change → session restart");
                            break;
                        }
                    }

                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                #[cfg(feature = "audio")]
                                if text.contains("audioToggle")
                                    && let Ok(SignalIn::AudioToggle { enabled }) =
                                        serde_json::from_str::<SignalIn>(&text)
                                {
                                    handle_client_audio_toggle(
                                        enabled,
                                        &audio_muted,
                                        opus_track.is_some(),
                                        device_id,
                                    );
                                    continue;
                                }
                                on_incoming_text(
                                    &text,
                                    &pc,
                                    &injector,
                                    screen_w,
                                    screen_h,
                                    &quality_tx,
                                    &settings_tx,
                                    scale_factor,
                                    &ws_out_tx,
                                    None,
                                ).await;
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                }
            }

            let _ = vp_shutdown_tx.send(());
            let _ = rtcp_shutdown_tx.send(());
            if let Some(tx) = av1_abr_shutdown_tx.take() {
                let _ = tx.send(());
            }
            #[cfg(feature = "audio")]
            if let Some(tx) = av1_ap_shutdown_tx.take() {
                let _ = tx.send(crate::desktop_audio_pipeline::StopReason::SessionClosed);
            }
            let _ = pc.close().await;
            return Ok(());
        }

        // ── JPEG path: desktop DC + 5-second fallback race ────────────────────
        //
        // Create the "desktop" DC (stream id=1) NOW — not at the top of the
        // session — because it is JPEG-only. Creating it unconditionally
        // pre-binds an SCTP stream the H.264 client never reserves, which
        // triggers a browser SCTP ERROR chunk and tears down the PC.
        //
        // Unordered + no retransmits — lowest latency for video tiles.
        let desktop_dc = pc
            .create_data_channel(
                "desktop",
                Some(RTCDataChannelInit {
                    ordered: Some(false),
                    max_retransmits: Some(0),
                    negotiated: Some(1),
                    ..Default::default()
                }),
            )
            .await?;

        // DC-open signal for the 5-s fallback race below. The on_open callback
        // can't pass the DC Arc directly, so we gate a oneshot.
        //
        // sink_chosen: 0=racing, 1=DC, 2=WS. Set by the race winner. The
        // on_open callback consults it so a DC that opens *after* the timer
        // already chose WS-fallback is closed immediately — leaving it open
        // would let SCTP buffers grow on a channel nothing ever drains.
        let sink_chosen = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let (dc_open_tx, mut dc_open_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let tx = Arc::new(std::sync::Mutex::new(Some(dc_open_tx)));
            let chosen = Arc::clone(&sink_chosen);
            let dc_for_close = Arc::clone(&desktop_dc);
            desktop_dc.on_open(Box::new(move || {
                let tx = Arc::clone(&tx);
                let chosen = Arc::clone(&chosen);
                let dc_for_close = Arc::clone(&dc_for_close);
                Box::pin(async move {
                    use std::sync::atomic::Ordering;
                    if chosen.load(Ordering::Acquire) == 2 {
                        // WS fallback already chosen — orphan DC, close it.
                        let _ = dc_for_close.close().await;
                        return;
                    }
                    if let Ok(mut guard) = tx.lock()
                        && let Some(s) = guard.take()
                    {
                        let _ = s.send(());
                    }
                })
            }));
        }

        // Channel for capture→WS binary frames (fallback path only).
        let (ws_bin_tx, mut ws_bin_rx) = mpsc::channel::<Bytes>(64);

        // Keep draining WS during the race so trickle-ICE candidates that
        // arrive immediately after the answer still reach the PeerConnection.
        // If they were deferred until ws_loop starts, the 5s timer often
        // wins and WebRTC falls back unnecessarily on LAN.
        let timer = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timer);
        let sink: Sink = loop {
            tokio::select! {
                biased;

                _ = &mut close_rx => {
                    return Err(anyhow::anyhow!("closed during dc-open race"));
                }

                Ok(()) = &mut dc_open_rx => {
                    sink_chosen.store(1, std::sync::atomic::Ordering::Release);
                    info!(device = %device_id, "desktop DC open — DataChannel path");
                    break Sink::DataChannel(Arc::clone(&desktop_dc));
                }

                _ = &mut timer => {
                    sink_chosen.store(2, std::sync::atomic::Ordering::Release);
                    info!(device = %device_id, "desktop DC timeout — WS fallback");
                    if let Ok(msg) = serde_json::to_string(&SignalOut::Fallback) {
                        let _ = socket.send(Message::Text(msg.into())).await;
                    }
                    // Best-effort close in case DC is already mid-handshake;
                    // on_open also guards against the post-timeout open race.
                    let _ = desktop_dc.close().await;
                    break Sink::WsBinary(ws_bin_tx);
                }

                // Drain pending outbound ICE candidates.
                Some(text) = ws_out_rx.recv() => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return Err(anyhow::anyhow!("WS write failed during race"));
                    }
                }

                // Apply incoming trickle-ICE during the race.
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(SignalIn::Ice { candidate }) =
                                serde_json::from_str::<SignalIn>(&text)
                                && let Err(e) = pc.add_ice_candidate(candidate).await
                            {
                                warn!(error = %e, "race-window ICE rejected");
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            return Err(anyhow::anyhow!("WS closed during dc-open race"));
                        }
                        _ => {}
                    }
                }
            }
        };

        // ── Capture pipeline ──────────────────────────────────────────────────
        let (cap_shutdown_tx, cap_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        // Capture → ws_loop signal: fires when capture exits unexpectedly so
        // the WS-fallback path can tell the client to reconnect instead of
        // hanging on a closed binary channel.
        let (cap_ended_tx, mut cap_ended_rx) = mpsc::channel::<String>(1);
        // Read initial tier + hidpi before moving the watches into the pipeline.
        let initial_tier = *quality_rx.borrow();
        let initial_hidpi_jpeg = *settings_rx.borrow();
        let settings_rx_for_pipeline = settings_tx.subscribe();

        // Emit Capabilities once, before the first binary tile leaves, so the
        // client can size its canvas from real encoder-output dimensions —
        // not from the HTTP `/desktop/capabilities` "is enabled" probe.
        let (out_w, out_h) = desktop::resize_dims(
            screen_w,
            screen_h,
            initial_tier,
            scale_factor,
            initial_hidpi_jpeg,
        );
        if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
            width: out_w,
            height: out_h,
            scale_factor,
            tile_size: desktop::TILE_SIZE,
        }) {
            let _ = ws_out_tx.send(msg).await;
        }

        spawn_capture_pipeline(
            initial_tier,
            initial_hidpi_jpeg,
            sink,
            quality_rx,
            settings_rx_for_pipeline,
            cap_shutdown_rx,
            scale_factor,
            cap_ended_tx,
            Arc::clone(&input_wake),
        );

        // ── Main WS event loop ────────────────────────────────────────────────
        ws_loop(
            &mut socket,
            &mut ws_out_rx,
            &mut ws_bin_rx,
            &pc,
            &injector,
            screen_w,
            screen_h,
            &quality_tx,
            &settings_tx,
            &mut close_rx,
            scale_factor,
            &ws_out_tx,
            &mut cap_ended_rx,
            &input_wake,
        )
        .await;

        // ── Teardown ──────────────────────────────────────────────────────────
        let _ = cap_shutdown_tx.send(());
        let _ = pc.close().await;
        Ok(())
    }

    // ── H.264 session branch ─────────────────────────────────────────────────

    /// Run a desktop session over an H.264 RTP media track. Replaces the
    /// JPEG DC-open race + `spawn_capture_pipeline` flow.
    ///
    /// Spawns three tasks:
    /// 1. `CaptureLoop::run_bgra` (blocking thread) — raw BGRA frames.
    /// 2. `video_pipeline::spawn_video_pipeline` — BGRA→H.264, writes RTP samples.
    /// 3. `video_pipeline::spawn_rtcp_reader` — PLI → force-IDR, REMB → bitrate.
    ///
    /// Waits for the first IDR's SPS/PPS, ships an `avcC` description over
    /// the signaling WS so the client's `VideoDecoder` can configure before
    /// the first RTP frame lands (avoids a WebCodecs error-and-recover stall).
    #[cfg(feature = "h264")]
    #[allow(clippy::too_many_arguments)]
    async fn run_h264_session(
        socket: &mut WebSocket,
        pc: &Arc<RTCPeerConnection>,
        ws_out_tx: &mpsc::Sender<String>,
        ws_out_rx: &mut mpsc::Receiver<String>,
        track: Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>,
        sender: Arc<RTCRtpSender>,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        scale_factor: f32,
        initial_hidpi: bool,
        // SPA detected loopback origin. Disables REMB → encoder bitrate
        // path so screen-share quality doesn't collapse on same-machine
        // view (Chrome's GCC misreads CPU jitter as network congestion).
        client_loopback: bool,
        quality_tx: &watch::Sender<QualityTier>,
        quality_rx: &mut watch::Receiver<QualityTier>,
        settings_tx: &watch::Sender<bool>,
        settings_rx: &mut watch::Receiver<bool>,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
        decision_reason: &'static str,
        device_id: &str,
        state: &Arc<AppState>,
        // Phase-02a: Some when the audio gate passed AND the Opus
        // transceiver was added before SRD. None elides the audio capture
        // path entirely — `run_bgra` is called with `audio_tx = None`, the
        // SCStream stays video-only, and `desktop_audio_pipeline` is never
        // spawned.
        #[cfg(feature = "audio")] audio_track: Option<
            Arc<webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample>,
        >,
        // User-level mute atomic. Flipped by mid-session `audioToggle` WS
        // messages from the client. Read by the audio writer task to drop
        // encoded frames instead of writing them to the track. Decoupled
        // from the operator master kill (`ap_shutdown_tx`) so user mute
        // never tears the pipeline down.
        #[cfg(feature = "audio")] audio_muted: Arc<std::sync::atomic::AtomicBool>,
    ) -> anyhow::Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64_STD, Engine as _};
        use desktop::capture::CaptureLoop;
        use desktop::encoders::BitrateBps;
        use desktop::resize_dims;
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

        let initial_tier = *quality_rx.borrow();
        let hidpi = initial_hidpi;
        let (out_w, out_h) = resize_dims(screen_w, screen_h, initial_tier, scale_factor, hidpi);

        // Phase-01 watchdog observability — these atomics let the fallback
        // path log a richer reason than "no-idr-5s" so Windows reports tell
        // us whether capture stalled, encoder rejected every frame, or
        // encoder produced frames but no IDR within the budget.
        let frames_captured = Arc::new(AtomicU64::new(0));
        let frames_encoded_ok = Arc::new(AtomicU64::new(0));
        let frames_encoded_err = Arc::new(AtomicU64::new(0));

        // Emit Capabilities so the client can size its <video> backing canvas.
        if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
            width: out_w,
            height: out_h,
            scale_factor,
            tile_size: desktop::TILE_SIZE,
        }) {
            let _ = ws_out_tx.send(msg).await;
        }

        // ── Channels ──────────────────────────────────────────────────────────
        let (bgra_tx, bgra_rx) = mpsc::channel::<desktop::RawBgraFrame>(2);
        let (bitrate_tx, bitrate_rx) = watch::channel(tier_bitrate(initial_tier, scale_factor, hidpi));
        let (pli_tx, pli_rx) = mpsc::channel::<()>(4);
        // Force an IDR after the host unlocks so the first frame after the
        // lock-screen overlay clears is clean (no decoder green-block from a
        // P-frame referencing pre-lock content). Spawns a tiny task that owns
        // its own lock-observer subscription + a clone of pli_tx.
        //
        // The subscription is a process-global broadcast whose sender never
        // closes, so `rx.recv()` never resolves to `None` — the task does NOT
        // exit on its own (dropping `pli_tx` only makes `try_send` start
        // failing; it does not wake the `rx.recv()` await). We therefore hold
        // the JoinHandle and abort it explicitly at session teardown.
        // Otherwise one of these tasks (plus its global subscriber entry)
        // leaks on every H.264 session, accumulating over days of reconnects.
        let lock_pli_task = {
            let pli_tx = pli_tx.clone();
            tokio::spawn(async move {
                let mut rx = desktop::macos_lock_observer::subscribe();
                while let Some(ev) = rx.recv().await {
                    if matches!(ev, desktop::macos_lock_observer::LockEvent::Unlocked) {
                        // try_send so a full pli queue (already-pending forced
                        // IDR) doesn't backpressure the observer; the encoder
                        // only needs one IDR per unlock anyway.
                        let _ = pli_tx.try_send(());
                    }
                }
            })
        };
        // Abort the lock→PLI task whenever this function returns (any break
        // out of the session loop, early `?`, or panic unwind).
        struct AbortOnDrop(tokio::task::JoinHandle<()>);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _lock_pli_guard = AbortOnDrop(lock_pli_task);
        let (params_tx, params_rx) = tokio::sync::oneshot::channel();
        let (vp_shutdown_tx, vp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (rtcp_shutdown_tx, rtcp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (cap_iframe_tx, cap_iframe_rx) = tokio::sync::oneshot::channel::<()>();
        // Force the first captured frame to be full — the encoder also forces
        // its first IDR, but firing here keeps behaviour symmetric with JPEG
        // where the starter tile-set is always complete.
        let _ = cap_iframe_tx.send(());

        // Phase-02a: when audio_track is Some, set up the audio mpsc + the
        // audio-pipeline shutdown oneshot. Capture-loop receives the tx side
        // and the SCK backend opens with audio. The rx side wraps into
        // SckAudioCapture, fed to spawn_audio_pipeline below.
        #[cfg(feature = "audio")]
        let (audio_tx, audio_rx_opt) = if audio_track.is_some() {
            // 8-slot cap matches the per-SCStream audio mpsc inside sck.rs —
            // total buffered audio across the two channels stays bounded at
            // ~160 ms before the pipeline starts dropping frames.
            let (tx, rx) = mpsc::channel::<Vec<i16>>(8);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        #[cfg(feature = "audio")]
        let (ap_shutdown_tx, ap_shutdown_rx) =
            tokio::sync::oneshot::channel::<crate::desktop_audio_pipeline::StopReason>();
        // Option-wrapped so the toggle-poll arm can `.take()` it for an
        // early teardown without consuming the value that teardown needs.
        #[cfg(feature = "audio")]
        let mut ap_shutdown_tx = Some(ap_shutdown_tx);

        // ── Spawn tasks ───────────────────────────────────────────────────────
        // Capture: resolution + HiDPI locked at session start (encoder dims are
        // fixed at init), but FPS tracks the live tier slider via `quality_rx`
        // so High delivers ~30 fps, Med ~15 fps, Low ~8 fps without restart.
        let cap_fps_rx = quality_tx.subscribe();
        #[cfg(feature = "audio")]
        let capture_audio_tx = audio_tx.clone();
        #[cfg(not(feature = "audio"))]
        let capture_audio_tx: Option<mpsc::Sender<Vec<i16>>> = None;
        let cap_frames_counter = Some(Arc::clone(&frames_captured));
        tokio::task::spawn_blocking(move || {
            CaptureLoop::run_bgra(
                initial_tier,
                cap_fps_rx,
                bgra_tx,
                scale_factor,
                hidpi,
                Some(cap_iframe_rx),
                capture_audio_tx,
                cap_frames_counter,
            )
        });

        // Audio pipeline: spawned when the gate passed (audio_track Some).
        // The helper is pipeline-agnostic — VP9 / AV1 dispatches reuse it.
        #[cfg(feature = "audio")]
        if let Some(track) = audio_track.as_ref() {
            try_spawn_session_audio_pipeline(
                track,
                audio_rx_opt,
                ap_shutdown_rx,
                &mut ap_shutdown_tx,
                Arc::clone(&audio_muted),
                state,
                device_id,
            );
        } else {
            let _ = audio_rx_opt;
            drop(ap_shutdown_rx);
        }
        // Drop the audio_tx clone we held only to thread into capture; the
        // capture loop owns its own clone now.
        #[cfg(feature = "audio")]
        drop(audio_tx);

        // Phase-03 step 2: ABR observation channel. Producers (encoder +
        // RTCP reader) push observations; consumers (controller + stats
        // SSE) attach in later steps. Broadcast capacity ≥ frame burst so
        // a slow consumer sees `Lagged` rather than blocking producers.
        let (abr_tx, _abr_rx_dropped) = crate::desktop_abr::channel();

        crate::video_pipeline::spawn_video_pipeline(crate::video_pipeline::VideoPipelineConfig {
            width: out_w,
            height: out_h,
            initial_bitrate: tier_bitrate(initial_tier, scale_factor, hidpi),
            initial_max_qp: tier_max_qp(initial_tier),
            track,
            bgra_rx,
            bitrate_rx,
            // Same watch source as the capture loop — writer paces RTP
            // sends to the user's selected fps so the burst-y SCK arrival
            // pattern doesn't fatten Chrome's jitter buffer.
            fps_rx: quality_tx.subscribe(),
            shutdown_rx: vp_shutdown_rx,
            pli_rx,
            params_tx,
            abr_tx: abr_tx.clone(),
            frames_encoded_ok: Some(Arc::clone(&frames_encoded_ok)),
            frames_encoded_err: Some(Arc::clone(&frames_encoded_err)),
        });

        crate::video_pipeline::spawn_rtcp_reader(
            sender,
            pli_tx,
            bitrate_tx.clone(),
            abr_tx.clone(),
            rtcp_shutdown_rx,
            // Loopback bypass: when true the REMB handler still emits the
            // observation (so stats SSE shows what Chrome thinks) but does
            // NOT push to bitrate_tx. Encoder stays at the operator's tier
            // bitrate, dodging GCC's loopback collapse.
            client_loopback,
        );
        if client_loopback {
            info!(
                device = %device_id,
                "h264: client signalled loopback origin — REMB → encoder bitrate disabled"
            );
        }

        // Phase-03 step 5: register the broadcast publisher with the
        // session registry so the stats SSE endpoint can subscribe by
        // device_id without reaching into pipeline internals. Optional
        // because non-desktop builds carry `Option<Arc<DesktopService>>`.
        if let Some(svc) = state.desktop_service.as_ref() {
            svc.attach_abr_tx(device_id, abr_tx.clone(), "h264");
        }

        // Phase-03 step 3: spawn the ABR controller. Skipped entirely on
        // loopback origins — same-machine paths have 0% loss + ~0 ms RTT,
        // so the controller has no signal to act on and would stay in
        // Comfort forever. The encoder keeps the operator's tier_bitrate
        // via the existing watch wiring; this matches the REMB-bypass
        // posture in `spawn_rtcp_reader`.
        let (abr_shutdown_tx, abr_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let mut abr_shutdown_tx = Some(abr_shutdown_tx);
        if !client_loopback {
            let initial_kbps = tier_bitrate(initial_tier, scale_factor, hidpi).0 / 1_000;
            // Floor = Low-tier preset (no HiDPI scaling) — controller can
            // cut bitrate aggressively in Recovery but never drops below a
            // value where the encoder produces visible artifacts. Ceiling =
            // current operator tier bitrate; mid-session tier changes
            // briefly override via the existing `quality_rx.changed()` arm,
            // and the controller re-establishes over its next few ticks.
            let floor_kbps = BitrateBps::LOW.0 / 1_000;
            crate::desktop_abr::spawn_controller(crate::desktop_abr::ControllerConfig {
                abr_rx: abr_tx.subscribe(),
                bitrate_tx: bitrate_tx.clone(),
                event_bus: Arc::clone(&state.event_bus),
                device_id: device_id.to_string(),
                floor_kbps,
                ceiling_kbps: initial_kbps,
                tunables: crate::desktop_abr::AbrTunables::from_env(),
                shutdown_rx: abr_shutdown_rx,
            });
        } else {
            // Drop the receiver so the channel doesn't hold a dangling
            // sender on shutdown teardown below. The Option-take keeps
            // teardown idempotent.
            drop(abr_shutdown_rx);
            abr_shutdown_tx = None;
            info!(
                device = %device_id,
                "h264: client signalled loopback origin — ABR controller skipped"
            );
        }

        info!(
            width = out_w,
            height = out_h,
            tier = ?initial_tier,
            hidpi,
            "h264 pipeline spawned"
        );

        // Track current HiDPI so a stray Settings echo with the same value
        // doesn't trigger a needless reconnect.
        let current_hidpi = hidpi;

        // ── Main WS loop, pumping signaling + watching for first IDR ──────────
        //
        // Phase-01 session-start watchdog: budget from
        // `RTCPeerConnectionState::Connected` to first IDR. Starting the
        // timer at session entry (before ICE) would false-positive on slow
        // cellular paths where ICE itself can take 3-5 s. We register a
        // PC-state-change callback that signals a Notify when Connected;
        // the watchdog future awaits the notify then sleeps. If the first
        // IDR arrives before the timer fires we set `params_rx = None` and
        // the watchdog arm becomes unselectable.
        //
        // Budget breakdown (worst case observed): PC connected → SCK init
        // ~450 ms → first BGRA frame with audio + HiDPI 3.3 MP up to ~3 s
        // → encode + params → ~50 ms. The original 3 s was too tight when
        // both audio and HiDPI were enabled, causing spurious JPEG
        // fallbacks. 5 s gives 2 s headroom while keeping the user wait
        // bounded.
        let mut params_rx = Some(params_rx);
        let connected_notify = Arc::new(tokio::sync::Notify::new());
        {
            // Only the first transition to Connected counts; PCs can briefly
            // bounce through Disconnected→Connected after ICE-restart in the
            // future, but we don't want to re-arm the watchdog mid-session
            // (params_rx is already None at that point anyway).
            let notify = Arc::clone(&connected_notify);
            let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
            pc.on_peer_connection_state_change(Box::new(move |state| {
                let notify = Arc::clone(&notify);
                let fired = Arc::clone(&fired);
                Box::pin(async move {
                    use std::sync::atomic::Ordering;
                    use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
                    if matches!(state, RTCPeerConnectionState::Connected)
                        && !fired.swap(true, Ordering::SeqCst)
                    {
                        notify.notify_waiters();
                    }
                })
            }));
        }
        let watchdog_notify = Arc::clone(&connected_notify);
        let watchdog = async move {
            watchdog_notify.notified().await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        };
        tokio::pin!(watchdog);

        // Phase-02a step 10b: detector future that resolves once when the
        // operator flips `desktop_audio_enabled` to false mid-session. Polls
        // settings every 2 s while the audio pipeline is live. When audio
        // never started (gate didn't pass) OR the feature is compiled out,
        // the future is pending forever — the arm below is a no-op. Single
        // future rather than a cfg-gated select arm because tokio::select!
        // doesn't parse `#[cfg]` on arms.
        let toggle_off_detector = async {
            #[cfg(feature = "audio")]
            if audio_track.is_some() {
                let mut i = tokio::time::interval(std::time::Duration::from_secs(2));
                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    i.tick().await;
                    if !crate::settings::get_desktop_audio_enabled(&state.db_path) {
                        return;
                    }
                }
            }
            std::future::pending::<()>().await;
        };
        tokio::pin!(toggle_off_detector);

        loop {
            tokio::select! {
                biased;

                _ = &mut *close_rx => break,

                // First IDR's SPS/PPS — ship the avcC description once.
                res = wait_params(params_rx.as_mut()), if params_rx.is_some() => {
                    params_rx = None;
                    if let Some(params) = res {
                        let hw_accel = params.is_hardware;
                        let avcc = desktop::build_avcc(&params.sps, &params.pps);
                        let b64 = B64_STD.encode(&avcc);
                        // `codec` is the *bitstream descriptor* the client
                        // sees post-negotiation (the SPA renders it in the
                        // status pill / telemetry). Derive from the SPS
                        // profile_idc so the label always matches what the
                        // encoder actually emits — macOS / VideoToolbox
                        // produces real High (0x64) while OpenH264 on the
                        // other targets stays Constrained Baseline (0x42).
                        let profile_idc = params.sps.get(1).copied().unwrap_or(0);
                        let codec_label = h264_codec_label_from_sps(profile_idc);
                        let msg = SignalOut::Pipeline {
                            mode: "h264",
                            codec: Some(codec_label),
                            tier_bitrates_kbps_low: BitrateBps::LOW.0 / 1_000,
                            tier_bitrates_kbps_med: BitrateBps::MED.0 / 1_000,
                            tier_bitrates_kbps_high: BitrateBps::HIGH.0 / 1_000,
                            avcc_description_b64: Some(b64),
                            reason: decision_reason,
                            hardware_accel: hw_accel,
                        };
                        if let Ok(txt) = serde_json::to_string(&msg) {
                            let _ = ws_out_tx.send(txt).await;
                        }
                        // SPS bytes 1-3 are profile_idc, constraint flags,
                        // level_idc — the same envelope that must match the
                        // SDP `profile-level-id`. Logging level_idc on first
                        // IDR makes the Windows black-frame diagnosis
                        // unambiguous: 0x32 = Level 5.0 (matches the
                        // `42e032` SDP fmtp), anything lower means the
                        // encoder is still emitting Cisco's auto-level.
                        let level_idc = params.sps.get(3).copied().unwrap_or(0);
                        let constraint = params.sps.get(2).copied().unwrap_or(0);
                        info!(
                            hardware_accel = hw_accel,
                            profile_idc = format_args!("0x{:02x}", profile_idc),
                            constraint_flags = format_args!("0x{:02x}", constraint),
                            level_idc = format_args!("0x{:02x}", level_idc),
                            "h264: avcC description sent, decoder can configure"
                        );
                    }
                }

                // Phase-01 watchdog: 3 s elapsed (post-Connected) without
                // an IDR. Send `FallbackPending` straight to the socket
                // (the mpsc would also work, but we want it on the wire
                // before we break out of the loop and tear the WS down)
                // and emit a `PipelineChosen` event tagging the JPEG
                // fallback so dashboards count it as a fallback session.
                _ = &mut watchdog, if params_rx.is_some() => {
                    let cap_n = frames_captured.load(AtomicOrdering::Relaxed);
                    let enc_ok = frames_encoded_ok.load(AtomicOrdering::Relaxed);
                    let enc_err = frames_encoded_err.load(AtomicOrdering::Relaxed);
                    // Classify the failure so Windows logs name a cause rather
                    // than just "no IDR". Wire reason stays short for telemetry
                    // stability; the warn log carries full counter values.
                    let wire_reason: &'static str = if cap_n == 0 {
                        "no-idr-5s-capture-stalled"
                    } else if enc_ok == 0 && enc_err > 0 {
                        "no-idr-5s-encode-errors"
                    } else if enc_ok == 0 {
                        "no-idr-5s-encode-skipped"
                    } else {
                        "no-idr-5s-no-keyframe"
                    };
                    warn!(
                        device = %device_id,
                        frames_captured = cap_n,
                        frames_encoded_ok = enc_ok,
                        frames_encoded_err = enc_err,
                        wire_reason,
                        "h264: session-start IDR watchdog tripped — signaling FallbackPending"
                    );
                    if let Ok(txt) = serde_json::to_string(&SignalOut::FallbackPending {
                        reason: wire_reason,
                    }) {
                        let _ = socket.send(Message::Text(txt.into())).await;
                    }
                    state.event_bus.send(AgentEvent::PipelineChosen {
                        device_id: device_id.to_string(),
                        pipeline: "jpeg".to_string(),
                        reason: "fallback-idr-watchdog".to_string(),
                    });
                    break;
                }

                // Push pending signaling (ICE + avcC) out to the WS client.
                Some(text) = ws_out_rx.recv() => {
                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                }

                // Tier change → bitrate update if dims unchanged, full
                // session restart if the new tier resizes capture. H.264
                // encoder dims are locked at init (VT can't resize live);
                // breaking lets the SPA reconnect rebuild at the new dims,
                // matching the hidpi-flip path below.
                Ok(_) = quality_rx.changed() => {
                    let new_tier = *quality_rx.borrow();
                    let (w, h) = resize_dims(
                        screen_w, screen_h, new_tier, scale_factor, current_hidpi,
                    );
                    if (w, h) != (out_w, out_h) {
                        info!(old = ?(out_w, out_h), new = ?(w, h), new_tier = ?new_tier,
                            "h264: tier change resizes capture → session restart");
                        break;
                    }
                    let _ = bitrate_tx.send(tier_bitrate(new_tier, scale_factor, current_hidpi));
                    if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                        width: w, height: h, scale_factor, tile_size: desktop::TILE_SIZE,
                    }) {
                        let _ = ws_out_tx.send(msg).await;
                    }
                }

                // HiDPI flip → encoder dims change. The encoder is locked at
                // init, so we can't resize live. Break out and let the caller
                // close the PC; the client's reconnect path then opens a new
                // session with the persisted setting (sent before offer).
                Ok(_) = settings_rx.changed() => {
                    let new_hidpi = *settings_rx.borrow();
                    if new_hidpi != current_hidpi {
                        info!(old = current_hidpi, new = new_hidpi,
                            "h264: hidpi change → session restart");
                        break;
                    }
                }

                // Phase-02a step 10b: operator flipped `desktop_audio_enabled`
                // to false mid-session. Hand the shutdown sender a
                // `UserToggleOff` reason and let the pipeline emit
                // `AudioStopped` from its single emit point. No-op on builds
                // without `feature = "audio"` (the detector future is pending
                // there). The arm is selectable forever, but `tokio::pin!`
                // means a resolved future stays resolved; the `take()` makes
                // re-entry a no-op so we don't double-fire on the same flip.
                _ = &mut toggle_off_detector => {
                    #[cfg(feature = "audio")]
                    if let Some(tx) = ap_shutdown_tx.take() {
                        info!(
                            device = %device_id,
                            "audio toggle flipped off mid-session — tearing down audio"
                        );
                        let _ = tx.send(
                            crate::desktop_audio_pipeline::StopReason::UserToggleOff,
                        );
                    }
                }

                // Incoming WS text: late ICE + ctrl-over-ws fallback.
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            // Client-driven audio mute (both directions —
                            // instant, no teardown, no renegotiation). Cheap
                            // substring pre-check avoids JSON parse on every
                            // input/quality/ICE msg. Flips the shared
                            // `audio_muted` atomic the writer task reads.
                            // The operator master-kill stays on
                            // `ap_shutdown_tx` via `toggle_off_detector`.
                            #[cfg(feature = "audio")]
                            if text.contains("audioToggle")
                                && let Ok(SignalIn::AudioToggle { enabled }) =
                                    serde_json::from_str::<SignalIn>(&text)
                            {
                                handle_client_audio_toggle(
                                    enabled,
                                    &audio_muted,
                                    audio_track.is_some(),
                                    device_id,
                                );
                                continue;
                            }
                            on_incoming_text(
                                &text,
                                pc,
                                injector,
                                screen_w,
                                screen_h,
                                quality_tx,
                                settings_tx,
                                scale_factor,
                                ws_out_tx,
                                None,
                            ).await;
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }
            }
        }

        // ── Teardown ──────────────────────────────────────────────────────────
        let _ = vp_shutdown_tx.send(());
        let _ = rtcp_shutdown_tx.send(());
        if let Some(tx) = abr_shutdown_tx.take() {
            let _ = tx.send(());
        }
        #[cfg(feature = "audio")]
        {
            // Audio pipeline teardown: fire shutdown so the encoder task exits
            // its capture.next_frame await without leaking the SckAudioCapture
            // (which holds the SCK audio mpsc rx). The pipeline owns the
            // `AudioStopped` emit point — passing `SessionClosed` lets it tag
            // the event with the right wire reason. The Option-take pattern
            // is a no-op if a mid-session toggle-off (UserToggleOff) already
            // consumed the sender; the pipeline already emitted in that case.
            if let Some(tx) = ap_shutdown_tx.take() {
                let _ = tx.send(crate::desktop_audio_pipeline::StopReason::SessionClosed);
            }
        }
        Ok(())
    }

    /// Map the SPS `profile_idc` byte (SPS[1] — byte 0 is the NAL header) to
    /// the human-readable codec label the SPA renders in its telemetry pill.
    /// Derived at runtime rather than hardcoded so a per-platform SDP fmtp
    /// split (VT on macOS = High, OpenH264 elsewhere = Constrained Baseline)
    /// stays in sync with the actual bitstream without `#[cfg]` here.
    #[cfg(feature = "h264")]
    fn h264_codec_label_from_sps(profile_idc: u8) -> &'static str {
        match profile_idc {
            0x64 => "h264-high-5.0",
            0x42 => "h264-baseline-3.1",
            _ => "h264-unknown",
        }
    }

    /// Poll the first-IDR oneshot. Returns `None` if the pipeline exits
    /// before producing a keyframe so the outer select doesn't hang.
    #[cfg(feature = "h264")]
    async fn wait_params(
        rx: Option<&mut tokio::sync::oneshot::Receiver<desktop::encoders::ParameterSets>>,
    ) -> Option<desktop::encoders::ParameterSets> {
        match rx {
            Some(r) => r.await.ok(),
            None => std::future::pending().await,
        }
    }

    /// Poll the VP9 first-IDR oneshot for sequence info. VP9 has no
    /// out-of-band parameter sets — the keyframe carries the sequence
    /// header inline — so this only surfaces `is_hardware` so the SPA pill
    /// can render `VP9 (HW)` vs `VP9 (SW)`.
    #[cfg(feature = "vp9")]
    async fn wait_vp9_seq_info(
        rx: Option<&mut tokio::sync::oneshot::Receiver<desktop::encoders::Vp9SequenceInfo>>,
    ) -> Option<desktop::encoders::Vp9SequenceInfo> {
        match rx {
            Some(r) => r.await.ok(),
            None => std::future::pending().await,
        }
    }

    /// Map quality tier + HiDPI flag to VP9 bitrate. **Phase-08 retune:**
    /// VP9 hits equivalent screen-content quality at ~40-50 % of H.264's
    /// bitrate per CRD measurements, so the presets drop accordingly —
    /// `LOW=1.0 Mbps`, `MED=2.5 Mbps`, `HIGH=5 Mbps` (vs H.264's
    /// `LOW=2.5 / MED=6 / HIGH=12 Mbps`). The 30 Mbps cap is preserved
    /// as a safety ceiling.
    ///
    /// **HiDPI area-scaling (2026-05-16):** in HiDPI mode the encoder
    /// receives physical pixels, so output pixel area grows as
    /// `scale_factor²` (e.g. 4× on a 2× Retina, 2.25× on Windows 1.5×).
    /// The bitrate budget tracks the area so bits-per-pixel stays
    /// constant across HiDPI on/off at the same tier. Previously a flat
    /// `× 2` left HiDPI sessions on Retina with half the budget per
    /// pixel — visibly smeared on motion and the in-video cursor lagged
    /// the SPA overlay sprite.
    #[cfg(feature = "vp9")]
    fn tier_bitrate_vp9(
        tier: QualityTier,
        scale_factor: f32,
        hidpi: bool,
    ) -> desktop::encoders::BitrateBps {
        use desktop::encoders::BitrateBps;
        let base = match tier {
            QualityTier::High => BitrateBps(5_000_000),
            QualityTier::Med => BitrateBps(2_500_000),
            QualityTier::Low => BitrateBps(1_000_000),
        };
        scale_hidpi_bitrate(base, scale_factor, hidpi)
    }

    /// Poll the AV1 first-IDR oneshot for sequence info. AV1 has no
    /// out-of-band parameter sets (the sequence OBU is the first OBU in the
    /// keyframe and rides inline through the RTP payloader).
    #[cfg(feature = "av1")]
    async fn wait_av1_seq_info(
        rx: Option<&mut tokio::sync::oneshot::Receiver<desktop::encoders::Av1SequenceInfo>>,
    ) -> Option<desktop::encoders::Av1SequenceInfo> {
        match rx {
            Some(r) => r.await.ok(),
            None => std::future::pending().await,
        }
    }

    /// Map quality tier + HiDPI flag to AV1 bitrate. **Phase-08 retune:**
    /// AV1 with `AOM_CONTENT_SCREEN` (palette + IBC) hits equivalent
    /// screen-content quality at ~30 % below VP9's bitrate. Presets
    /// drop to `LOW=0.7 Mbps`, `MED=1.5 Mbps`, `HIGH=3 Mbps`. Matches the
    /// bitrate footprint Chrome Remote Desktop uses for AV1 sessions.
    ///
    /// HiDPI scales by `scale_factor²` — see `tier_bitrate_vp9` for rationale.
    #[cfg(feature = "av1")]
    fn tier_bitrate_av1(
        tier: QualityTier,
        scale_factor: f32,
        hidpi: bool,
    ) -> desktop::encoders::BitrateBps {
        use desktop::encoders::BitrateBps;
        let base = match tier {
            QualityTier::High => BitrateBps(3_000_000),
            QualityTier::Med => BitrateBps(1_500_000),
            QualityTier::Low => BitrateBps(700_000),
        };
        scale_hidpi_bitrate(base, scale_factor, hidpi)
    }

    /// Handle a client `audioToggle` signal mid-session. Flips the shared
    /// mute atomic the audio writer task reads — instant in both
    /// directions, no PC renegotiation, no SCStream rebuild, no audio
    /// pipeline teardown. Operator-level master kill (settings flip) stays
    /// on `ap_shutdown_tx` via `toggle_off_detector`. No-op when the audio
    /// gate didn't pass at session-start (no transceiver to unmute into);
    /// the SPA never shows the toggle in that case, but defensive logging
    /// helps diagnose stale clients.
    #[cfg(feature = "audio")]
    fn handle_client_audio_toggle(
        enabled: bool,
        audio_muted: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        audio_gate_passed: bool,
        device_id: &str,
    ) {
        if !audio_gate_passed {
            tracing::debug!(
                device = %device_id,
                requested_enabled = enabled,
                "audioToggle ignored: audio gate did not pass at session-start"
            );
            return;
        }
        let muted = !enabled;
        audio_muted.store(muted, std::sync::atomic::Ordering::Relaxed);
        info!(
            device = %device_id,
            muted,
            "client audioToggle applied (atomic flip, no teardown)"
        );
    }

    /// Spawn the audio capture + Opus encoder pipeline for the current
    /// session. Pipeline-agnostic — used by all video-track sessions
    /// (H.264 / VP9 / AV1). Caller has already passed the audio
    /// mpsc tx side into the capture loop; this helper picks up the rx
    /// side (macOS SCK bridge) or opens an independent WASAPI loopback
    /// (Windows) and hands the result to `spawn_audio_pipeline`.
    ///
    /// On capture-init failure the audio transceiver stays on the PC but
    /// receives no frames. We emit `AudioStopped` so dashboards reflect
    /// reality and consume the shutdown sender slot so the later teardown
    /// path doesn't try to fire `SessionClosed` into a pipeline that never
    /// started.
    #[cfg(feature = "audio")]
    fn try_spawn_session_audio_pipeline(
        audio_track: &std::sync::Arc<
            webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample,
        >,
        audio_rx_opt: Option<tokio::sync::mpsc::Receiver<Vec<i16>>>,
        ap_shutdown_rx: tokio::sync::oneshot::Receiver<
            crate::desktop_audio_pipeline::StopReason,
        >,
        ap_shutdown_tx: &mut Option<
            tokio::sync::oneshot::Sender<crate::desktop_audio_pipeline::StopReason>,
        >,
        muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
        state: &std::sync::Arc<AppState>,
        device_id: &str,
    ) {
        #[cfg(target_os = "windows")]
        let capture_error_reason = crate::desktop_audio_pipeline::StopReason::WasapiError;
        #[cfg(not(target_os = "windows"))]
        let capture_error_reason = crate::desktop_audio_pipeline::StopReason::SckError;

        let capture_init: Result<
            Box<dyn desktop::audio::AudioCapture>,
            desktop::audio::AudioError,
        > = {
            #[cfg(target_os = "macos")]
            {
                match audio_rx_opt {
                    Some(rx) => Ok(Box::new(
                        desktop::audio::sck_audio::SckAudioCapture::from_receiver(rx),
                    )),
                    None => Err(desktop::audio::AudioError::CaptureFailed(
                        "sck audio rx absent".into(),
                    )),
                }
            }
            #[cfg(target_os = "windows")]
            {
                let _ = audio_rx_opt;
                desktop::audio::make_default()
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                let _ = audio_rx_opt;
                Err(desktop::audio::AudioError::Unsupported(
                    "audio.platform.unsupported",
                ))
            }
        };

        match capture_init {
            Ok(capture) => {
                crate::desktop_audio_pipeline::spawn_audio_pipeline(
                    crate::desktop_audio_pipeline::AudioPipelineConfig {
                        track: audio_track.clone(),
                        capture,
                        shutdown_rx: ap_shutdown_rx,
                        event_bus: std::sync::Arc::clone(&state.event_bus),
                        device_id: device_id.to_string(),
                        capture_error_reason,
                        muted,
                    },
                );
                state
                    .event_bus
                    .send(crate::events::AgentEvent::AudioStarted {
                        device_id: device_id.to_string(),
                    });
                state.telemetry.record_audio_session_started();
                info!("audio pipeline spawned (Opus → RTP)");
            }
            Err(err) => {
                warn!(error = %err, "audio capture init failed; pipeline not spawned");
                state
                    .event_bus
                    .send(crate::events::AgentEvent::AudioStopped {
                        device_id: device_id.to_string(),
                        reason: capture_error_reason.as_wire().to_string(),
                    });
                let _ = ap_shutdown_tx.take();
                drop(ap_shutdown_rx);
            }
        }
    }

    /// Map a quality tier to its VideoToolbox `MaxAllowedFrameQP` cap.
    /// H.264 QP is 0-51 (0=lossless, 51=worst). Tier mapping:
    /// - HIGH → Some(23): "visually-OK" floor for text-heavy UI. Encoder
    ///   must spend bits to keep QP ≤ 23. At 24 Mbps HiDPI HIGH this is
    ///   sustainable on screen content.
    /// - MED  → Some(28): "OK natural video" — text mildly soft but no
    ///   real-bandwidth-starvation drops at 12 Mbps.
    /// - LOW  → None: bandwidth-first; preserving frame rate matters
    ///   more than text crispness at the LOW tier preset.
    ///
    /// Session-lifetime constant — VT does not support live QP-cap
    /// updates without session rebuild. Mid-session tier changes update
    /// only bitrate via `bitrate_tx`; the QP cap stays at whatever
    /// `initial_tier` selected. Acceptable since users rarely flip
    /// between LOW and HIGH mid-session.
    #[cfg(feature = "h264")]
    fn tier_max_qp(tier: QualityTier) -> Option<u32> {
        match tier {
            QualityTier::High => Some(23),
            QualityTier::Med => Some(28),
            QualityTier::Low => None,
        }
    }

    /// Map a quality tier (and HiDPI flag) to its H.264 bitrate preset. Keeps
    /// the bitrate table in one place so the `SignalOut::Pipeline` message
    /// and the encoder never drift apart.
    ///
    /// HiDPI doubles the encoder pixel count (~4× actually, but H.264
    /// compresses near-static UI well). The 2× bitrate multiplier captures
    /// most of the extra detail without overshooting REMB's typical ceiling.
    /// Capped at 30 Mbps (raised 2026-05-13 from 20 Mbps as part of the
    /// `260513-0009-h264-quality-uplift-vt-high-profile` plan) to land in
    /// Sunshine/Moonlight territory for Retina captures. ABR's Recovery
    /// zone still cuts 30% on loss; floor stays at LOW tier (2.5 Mbps) so
    /// cellular sessions converge to a sane bitrate even at the new cap.
    #[cfg(feature = "h264")]
    fn tier_bitrate(
        tier: QualityTier,
        scale_factor: f32,
        hidpi: bool,
    ) -> desktop::encoders::BitrateBps {
        use desktop::encoders::BitrateBps;
        let base = match tier {
            QualityTier::High => BitrateBps::HIGH,
            QualityTier::Med => BitrateBps::MED,
            QualityTier::Low => BitrateBps::LOW,
        };
        scale_hidpi_bitrate(base, scale_factor, hidpi)
    }

    /// Shared HiDPI bitrate scaling for H.264/VP9/AV1. In HiDPI mode the
    /// encoder receives physical pixels, so output area grows as
    /// `scale_factor²`. We track that to keep bits-per-pixel constant
    /// across HiDPI on/off at the same tier, capped at 30 Mbps.
    /// `scale_factor` is clamped to `>= 1.0` defensively — values below
    /// 1.0 don't make physical sense for HiDPI but would otherwise
    /// shrink the budget.
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    fn scale_hidpi_bitrate(
        base: desktop::encoders::BitrateBps,
        scale_factor: f32,
        hidpi: bool,
    ) -> desktop::encoders::BitrateBps {
        use desktop::encoders::BitrateBps;
        const CAP_BPS: f64 = 30_000_000.0;
        if !hidpi {
            return base;
        }
        let sf = (scale_factor as f64).max(1.0);
        let scaled = (base.0 as f64 * sf * sf).min(CAP_BPS).round();
        BitrateBps(scaled as u32)
    }

    // ── Signaling: offer → answer ─────────────────────────────────────────────

    /// Read WS messages until an "offer" arrives. Does NOT process the offer
    /// — the caller needs to build a `RTCPeerConnection` with the right
    /// `MediaEngine` codec set AFTER learning the pipeline choice.
    ///
    /// Pre-offer ICE candidates are buffered and returned so the caller can
    /// apply them to the freshly-built PC after construction. This matters
    /// because PC construction happens after we know the pipeline (from
    /// `CapabilitiesClient`+`Offer`), and ICE may arrive before then.
    ///
    /// Also collects any pre-offer `Settings` so the encoder starts with
    /// the user's persisted HiDPI preference without a reconnect round-trip.
    ///
    /// Returns: `(pipeline, reason, offer_sdp, initial_hidpi, client_audio,
    ///            client_loopback, buffered_ice_candidates)`.
    ///
    /// `operator` is the per-session preference (env var unless overridden
    /// by `?force_pipeline=` on the WS upgrade). When the operator forces a
    /// pipeline the client can't decode, ships `CaptureEnded` and returns
    /// `Err` (fail-closed, no silent JPEG fallback).
    /// Case-insensitive scan of an offer SDP's `a=rtpmap:` lines for a codec
    /// name. The browser-side `RTCRtpReceiver.getCapabilities('video')` and
    /// the actual codecs it lists in `pc.createOffer()` can disagree on
    /// some codecs (notably AV1 in older Chrome builds). Calling
    /// `set_local_description` then fails inside `TrackLocalStaticSample::bind`
    /// with `ErrUnsupportedCodec`, and the session error-closes with no
    /// graceful fallback. Validate before we commit.
    fn offer_advertises_codec(sdp: &str, codec_name: &str) -> bool {
        // SDP rtpmap line: `a=rtpmap:<PT> <NAME>/<CLOCK>[/<CHANNELS>]`.
        // We only need a fuzzy substring check — `VP9/90000`, `AV1/90000`,
        // `H264/90000`. Uppercase normalise both sides.
        let needle = format!("{}/90000", codec_name.to_ascii_uppercase());
        sdp.lines().any(|line| {
            let l = line.trim();
            if !l.to_ascii_lowercase().starts_with("a=rtpmap:") {
                return false;
            }
            l.to_ascii_uppercase().contains(&needle)
        })
    }

    /// Map a `Pipeline` to the SDP codec name browsers use in their offer's
    /// `a=rtpmap:` line. JPEG has no SDP codec (uses DataChannel transport).
    #[allow(unused_variables, unreachable_patterns)] // codec arms are cfg-gated
    fn pipeline_sdp_codec(pipeline: Pipeline) -> Option<&'static str> {
        match pipeline {
            #[cfg(feature = "h264")]
            Pipeline::H264 => Some("H264"),
            #[cfg(feature = "vp9")]
            Pipeline::Vp9 => Some("VP9"),
            #[cfg(feature = "av1")]
            Pipeline::Av1 => Some("AV1"),
            Pipeline::Jpeg => None,
        }
    }

    async fn await_offer_with_caps(
        socket: &mut WebSocket,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
        operator: OperatorPref,
        quality_tx: &watch::Sender<QualityTier>,
    ) -> anyhow::Result<(
        Pipeline,
        &'static str,
        String,
        bool,
        bool,
        bool,
        Vec<RTCIceCandidateInit>,
    )> {
        let mut client_caps = ClientCapabilities::default();
        let mut initial_hidpi = false;
        let mut buffered_ice: Vec<RTCIceCandidateInit> = Vec::new();

        loop {
            tokio::select! {
                biased;

                _ = &mut *close_rx => {
                    return Err(anyhow::anyhow!("closed during negotiation"));
                }

                msg = socket.recv() => {
                    let Some(Ok(Message::Text(text))) = msg else {
                        return Err(anyhow::anyhow!("WS closed during negotiation"));
                    };
                    match serde_json::from_str::<SignalIn>(&text) {
                        Ok(SignalIn::Offer { sdp }) => {
                            match choose_pipeline(operator, &client_caps) {
                                Ok(decision) => {
                                    // Second-stage validation: the client's
                                    // capabilitiesClient.codecs list reflects
                                    // RTCRtpReceiver.getCapabilities, which can
                                    // overstate WebRTC RTP support. Confirm the
                                    // chosen codec is actually in the offer's
                                    // m=video rtpmap — otherwise webrtc-rs's
                                    // TrackLocalStaticSample bind fails with
                                    // ErrUnsupportedCodec mid-negotiation and
                                    // the WS error-closes with no graceful
                                    // fallback. FallbackPending
                                    // lets the SPA mount JPEG instead.
                                    if let Some(codec_name) = pipeline_sdp_codec(decision.pipeline)
                                        && !offer_advertises_codec(&sdp, codec_name)
                                    {
                                        warn!(
                                            pipeline = decision.pipeline.wire_name(),
                                            codec_name,
                                            "offer SDP missing chosen codec's rtpmap — client overadvertised; falling back"
                                        );
                                        let reason: String = format!(
                                            "{}-not-in-offer-sdp",
                                            decision.pipeline.wire_name()
                                        );
                                        if let Ok(msg) = serde_json::to_string(
                                            &SignalOut::FallbackPending {
                                                reason: reason.as_str(),
                                            },
                                        ) {
                                            let _ = socket
                                                .send(Message::Text(msg.into()))
                                                .await;
                                        }
                                        return Err(anyhow::anyhow!(
                                            "{} not in offer SDP",
                                            codec_name
                                        ));
                                    }
                                    info!(
                                        operator = ?operator,
                                        client_webcodecs = client_caps.webcodecs,
                                        client_codec_count = client_caps.codecs.len(),
                                        pipeline = decision.pipeline.wire_name(),
                                        reason = decision.reason,
                                        initial_hidpi,
                                        "negotiation: offer received"
                                    );
                                    return Ok((
                                        decision.pipeline,
                                        decision.reason,
                                        sdp,
                                        initial_hidpi,
                                        client_caps.audio,
                                        client_caps.loopback,
                                        buffered_ice,
                                    ));
                                }
                                Err(err) => {
                                    // Operator forced a pipeline but the client
                                    // can't decode it. Tell the client clearly
                                    // before tearing down — the SPA's
                                    // `captureEnded` handler surfaces the
                                    // reason in the reconnect modal.
                                    warn!(
                                        operator = ?operator,
                                        reason = %err,
                                        "pipeline forced but unsatisfiable"
                                    );
                                    if let Ok(msg) =
                                        serde_json::to_string(&SignalOut::CaptureEnded {
                                            reason: format!(
                                                "Server pipeline forced but client lacks decode support ({})",
                                                err
                                            ),
                                        })
                                    {
                                        let _ =
                                            socket.send(Message::Text(msg.into())).await;
                                    }
                                    return Err(anyhow::anyhow!("{}", err));
                                }
                            }
                        }
                        Ok(SignalIn::Ice { candidate }) => {
                            // Buffer pre-offer ICE candidates. The PC is built
                            // AFTER capability negotiation; the caller applies
                            // them once the PC exists.
                            buffered_ice.push(candidate);
                        }
                        Ok(SignalIn::CapabilitiesClient { codecs, webcodecs, audio, loopback }) => {
                            client_caps.codecs = codecs;
                            client_caps.webcodecs = webcodecs;
                            client_caps.audio = audio;
                            client_caps.loopback = loopback;
                        }
                        Ok(SignalIn::Settings { hidpi }) => {
                            initial_hidpi = hidpi;
                        }
                        Ok(SignalIn::Quality { tier }) => {
                            // Pre-offer tier hint — write straight into the
                            // session-wide `quality_tx` so the H.264/VP9/AV1
                            // dispatchers read it as `initial_tier`. Unknown
                            // values are ignored (channel stays at default
                            // Med); the post-offer ctrl-DC `quality` message
                            // still corrects it via the live-update path.
                            if let Some(t) = parse_quality_tier(&tier) {
                                let _ = quality_tx.send(t);
                            }
                        }
                        Ok(SignalIn::AudioToggle { .. }) => {
                            // Pre-offer phase: the audio pipeline isn't
                            // spawned yet (it boots inside `run_h264_session`
                            // after SDP exchange). Ignoring keeps clients
                            // from racing the toggle ahead of the offer.
                            // Post-session toggle is honoured in the WS arm
                            // of `run_h264_session` once the pipeline lives.
                        }
                        Err(e) => warn!(error = %e, "unknown signaling message — ignored"),
                    }
                }
            }
        }
    }

    /// Set the remote offer, optionally add a sending track (so the answer
    /// includes a matching m-line), create + set local answer, and send it.
    ///
    /// `sending_track` is Some only when the Pipeline chose H.264 and the
    /// caller has already constructed the `TrackLocalStaticSample`. The
    /// returned `Arc<RTCRtpSender>` is needed by `spawn_rtcp_reader` to
    /// parse PLI and REMB feedback.
    async fn complete_answer(
        pc: &RTCPeerConnection,
        offer_sdp: String,
        ws_out_tx: &mpsc::Sender<String>,
        sending_track: Option<Arc<dyn TrackLocal + Send + Sync>>,
        audio_sending_track: Option<Arc<dyn TrackLocal + Send + Sync>>,
    ) -> anyhow::Result<Option<Arc<RTCRtpSender>>> {
        // Pre-bind the sendonly transceiver BEFORE set_remote_description so
        // `satisfy_type_and_direction` pairs it with the client's recvonly
        // m=video. `add_track()` after set_remote_description never reuses
        // the pending transceiver (webrtc-rs 0.11 only matches when the
        // track id equals the sender id — never true for fresh tracks), so
        // it spawns a second transceiver that doesn't map to any m-line and
        // the frames we write go nowhere. This change moves the binding
        // earlier so the single mid:0 m-line in the answer carries our RTP.
        //
        // Audio track (phase-02a): same gotcha applies. The Opus transceiver
        // must be added BEFORE SRD so the BUNDLE'd answer carries an audio
        // m-line matching the client's recvonly request. Order between
        // video + audio doesn't matter for SDP correctness, but we bind
        // video first to keep the video-only `rtp_sender` returned below
        // unaffected.
        let rtp_sender = if let Some(track) = sending_track {
            let transceiver = pc
                .add_transceiver_from_track(
                    track,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        send_encodings: vec![],
                    }),
                )
                .await?;
            Some(transceiver.sender().await)
        } else {
            None
        };
        if let Some(audio_track) = audio_sending_track {
            pc.add_transceiver_from_track(
                audio_track,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Sendonly,
                    send_encodings: vec![],
                }),
            )
            .await?;
        }
        pc.set_remote_description(RTCSessionDescription::offer(offer_sdp)?)
            .await?;
        let answer = pc.create_answer(None).await?;
        let sdp_out = answer.sdp.clone();
        // Log every H.264 fmtp line in the answer so the SDP envelope
        // negotiated for this session is visible without running tcpdump or
        // chrome://webrtc-internals. Crucial for verifying that
        // `profile-level-id` matches what the encoder actually emits — a
        // mismatch is what makes Chrome MF on Windows render black frames.
        for line in sdp_out.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("a=fmtp:")
                && lower.contains("profile-level-id")
            {
                info!(fmtp = line, "answer SDP fmtp");
            }
        }
        pc.set_local_description(answer).await?;
        let msg = serde_json::to_string(&SignalOut::Answer { sdp: &sdp_out })?;
        ws_out_tx.send(msg).await?;
        Ok(rtp_sender)
    }

    // ── Post-negotiation WS event loop ────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    async fn ws_loop(
        socket: &mut WebSocket,
        ws_out_rx: &mut mpsc::Receiver<String>,
        ws_bin_rx: &mut mpsc::Receiver<Bytes>,
        pc: &RTCPeerConnection,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        quality_tx: &watch::Sender<QualityTier>,
        settings_tx: &watch::Sender<bool>,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
        scale_factor: f32,
        ws_out_tx: &mpsc::Sender<String>,
        cap_ended_rx: &mut mpsc::Receiver<String>,
        input_wake: &desktop::capture::InputWake,
    ) {
        loop {
            tokio::select! {
                biased;

                _ = &mut *close_rx => return,

                // Capture pipeline exited unexpectedly — surface the reason
                // to the client so it can show a reconnect modal instead of
                // staring at a frozen frame, then tear down the WS.
                Some(reason) = cap_ended_rx.recv() => {
                    if let Ok(msg) = serde_json::to_string(&SignalOut::CaptureEnded { reason }) {
                        let _ = socket.send(Message::Text(msg.into())).await;
                    }
                    return;
                }

                // Forward outgoing ICE to the WS text channel.
                Some(text) = ws_out_rx.recv() => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }

                // Forward binary frames (WS-fallback path).
                Some(bin) = ws_bin_rx.recv() => {
                    if socket.send(Message::Binary(bin.to_vec().into())).await.is_err() {
                        return;
                    }
                }

                // Handle incoming WS messages (late ICE, fallback ctrl events).
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            on_incoming_text(
                                &text,
                                pc,
                                injector,
                                screen_w,
                                screen_h,
                                quality_tx,
                                settings_tx,
                                scale_factor,
                                ws_out_tx,
                                Some(input_wake),
                            )
                            .await;
                        }
                        Some(Ok(Message::Close(_))) | None => return,
                        _ => {}
                    }
                }
            }
        }
    }

    /// Handle a post-negotiation WS text message.
    ///
    /// Could be a late ICE candidate, a Settings update from the persistent
    /// client preference channel, or a ctrl event arriving over WS in
    /// fallback mode (when the DataChannel never opened).
    #[allow(clippy::too_many_arguments)]
    async fn on_incoming_text(
        text: &str,
        pc: &RTCPeerConnection,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        quality_tx: &watch::Sender<QualityTier>,
        settings_tx: &watch::Sender<bool>,
        scale_factor: f32,
        ws_out_tx: &mpsc::Sender<String>,
        input_wake: Option<&desktop::capture::InputWake>,
    ) {
        // ICE / Settings — handled inline. Settings on the WS text channel
        // covers the case where the client wants to push a preference change
        // before the ctrl DC has opened (or in WS-fallback mode).
        match serde_json::from_str::<SignalIn>(text) {
            Ok(SignalIn::Ice { candidate }) => {
                if let Err(e) = pc.add_ice_candidate(candidate).await {
                    warn!(error = %e, "post-offer ICE candidate rejected");
                }
                return;
            }
            Ok(SignalIn::Settings { hidpi }) => {
                let _ = settings_tx.send(hidpi);
                return;
            }
            _ => {}
        }

        // Ctrl / input event (fallback mode — DC never opened)?
        // dispatch_input parses internally; only call it when the text is not
        // a signaling message (handled above). Wake the capture loop on any
        // ctrl traffic so idle backoff snaps to active cadence.
        if let Some(wake) = input_wake {
            wake.notify_one();
        }
        dispatch_input(
            text,
            injector,
            screen_w,
            screen_h,
            quality_tx,
            settings_tx,
            scale_factor,
            Some(ws_out_tx),
        )
        .await;
    }

    /// Parse a ctrl-channel message and dispatch to InputInjector, quality_tx,
    /// or settings_tx.
    ///
    /// When `caps_tx` is `Some`, a tier change also re-emits `Capabilities`
    /// over the WS text channel so the client can resize its canvas to the
    /// new encoder output grid.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_input(
        text: &str,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        quality_tx: &watch::Sender<QualityTier>,
        settings_tx: &watch::Sender<bool>,
        scale_factor: f32,
        caps_tx: Option<&mpsc::Sender<String>>,
    ) {
        let wire: WireInput = match serde_json::from_str(text) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "ctrl: invalid input JSON");
                return;
            }
        };

        // Quality + Settings are handled before InputEvent conversion: they
        // route to watch channels, not the input injector.
        match &wire {
            WireInput::Quality { tier } => {
                if let Some(t) = parse_quality_tier(tier) {
                    info!(tier = ?t, "quality change requested");
                    let _ = quality_tx.send(t);
                    if let Some(tx) = caps_tx {
                        let hidpi = *settings_tx.borrow();
                        let (w, h) =
                            desktop::resize_dims(screen_w, screen_h, t, scale_factor, hidpi);
                        if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                            width: w,
                            height: h,
                            scale_factor,
                            tile_size: desktop::TILE_SIZE,
                        }) {
                            let _ = tx.send(msg).await;
                        }
                    }
                }
                return;
            }
            WireInput::Settings { hidpi } => {
                info!(hidpi, "settings change requested (hidpi)");
                let _ = settings_tx.send(*hidpi);
                if let Some(tx) = caps_tx {
                    let tier = *quality_tx.borrow();
                    let (w, h) =
                        desktop::resize_dims(screen_w, screen_h, tier, scale_factor, *hidpi);
                    if let Ok(msg) = serde_json::to_string(&SignalOut::Capabilities {
                        width: w,
                        height: h,
                        scale_factor,
                        tile_size: desktop::TILE_SIZE,
                    }) {
                        let _ = tx.send(msg).await;
                    }
                }
                return;
            }
            _ => {}
        }

        let Some(event) = wire.into_input_event() else { return };

        if let Some(inj) = injector {
            let mut guard = inj.lock().await;
            if let Err(e) = guard.apply(event, screen_w, screen_h) {
                warn!(error = %e, "InputInjector::apply failed");
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Get native dimensions of the primary monitor for input coordinate mapping.
    /// Falls back to 1920×1080 if the monitor cannot be probed.
    fn primary_screen_dimensions() -> (u32, u32) {
        desktop::list_monitors()
            .into_iter()
            .next()
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| (m.width, m.height))
            .unwrap_or((1920, 1080))
    }

    // ─── Tests ───────────────────────────────────────────────────────────────
    #[cfg(test)]
    mod wire_input_tests {
        use super::*;

        /// Wire-format guard: `{t:'text', s:...}` parses into `WireInput::Text`
        /// and round-trips through `into_input_event` to `InputEvent::Text`.
        /// Catches accidental tag-rename or struct-shape regressions.
        #[test]
        fn text_event_parses_from_wire() {
            let json = r#"{"t":"text","s":"Hello!@#"}"#;
            let parsed: WireInput = serde_json::from_str(json).expect("parse");
            match &parsed {
                WireInput::Text { s } => assert_eq!(s, "Hello!@#"),
                other => panic!("expected Text variant, got {other:?}"),
            }
            match parsed.into_input_event() {
                Some(InputEvent::Text { s }) => assert_eq!(s, "Hello!@#"),
                other => panic!("expected InputEvent::Text, got {other:?}"),
            }
        }
    }

    #[cfg(all(test, feature = "h264"))]
    mod h264_codec_label_tests {
        use super::*;

        /// SPS `profile_idc` → telemetry codec string. macOS VT emits 0x64
        /// (High); OpenH264 emits 0x42 (Baseline); anything else gets the
        /// defensive `h264-unknown` fall-through so a future encoder can't
        /// silently mislabel without surfacing in telemetry.
        #[test]
        fn label_maps_known_profiles() {
            assert_eq!(h264_codec_label_from_sps(0x64), "h264-high-5.0");
            assert_eq!(h264_codec_label_from_sps(0x42), "h264-baseline-3.1");
            assert_eq!(h264_codec_label_from_sps(0x4d), "h264-unknown"); // Main
            assert_eq!(h264_codec_label_from_sps(0x00), "h264-unknown");
        }
    }

    #[cfg(test)]
    mod offer_codec_scan_tests {
        use super::*;

        const MINIMAL_OFFER_H264_ONLY: &str = "\
v=0\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=fmtp:96 profile-level-id=42e01f\r\n";

        const MINIMAL_OFFER_H264_VP9: &str = "\
v=0\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96 98\r\n\
a=rtpmap:96 H264/90000\r\n\
a=rtpmap:98 VP9/90000\r\n";

        const OFFER_WITH_AV1: &str = "\
v=0\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96 41\r\n\
a=rtpmap:96 H264/90000\r\n\
a=rtpmap:41 AV1/90000\r\n";

        #[test]
        fn detects_codec_present() {
            assert!(offer_advertises_codec(MINIMAL_OFFER_H264_ONLY, "H264"));
            assert!(offer_advertises_codec(MINIMAL_OFFER_H264_VP9, "VP9"));
            assert!(offer_advertises_codec(OFFER_WITH_AV1, "AV1"));
        }

        #[test]
        fn rejects_codec_absent() {
            assert!(!offer_advertises_codec(MINIMAL_OFFER_H264_ONLY, "AV1"));
            assert!(!offer_advertises_codec(MINIMAL_OFFER_H264_VP9, "AV1"));
            assert!(!offer_advertises_codec(MINIMAL_OFFER_H264_ONLY, "VP9"));
        }

        #[test]
        fn case_insensitive_match() {
            let lower = "a=rtpmap:41 av1/90000\r\n";
            let upper = "a=rtpmap:41 AV1/90000\r\n";
            assert!(offer_advertises_codec(lower, "AV1"));
            assert!(offer_advertises_codec(upper, "av1"));
        }

        /// rtpmap is the only place the codec name appears canonically — a=fmtp
        /// payload-level params or a stray comment must not false-positive.
        #[test]
        fn ignores_non_rtpmap_lines() {
            let sdp = "\
m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=fmtp:96 profile-level-id=42e01f AV1-MENTION-IN-FMTP\r\n";
            assert!(!offer_advertises_codec(sdp, "AV1"));
        }

        #[test]
        fn jpeg_pipeline_has_no_sdp_codec() {
            assert_eq!(pipeline_sdp_codec(Pipeline::Jpeg), None);
        }

        #[cfg(feature = "h264")]
        #[test]
        fn h264_pipeline_maps_to_h264_codec_name() {
            assert_eq!(pipeline_sdp_codec(Pipeline::H264), Some("H264"));
        }

        #[cfg(feature = "vp9")]
        #[test]
        fn vp9_pipeline_maps_to_vp9_codec_name() {
            assert_eq!(pipeline_sdp_codec(Pipeline::Vp9), Some("VP9"));
        }

        #[cfg(feature = "av1")]
        #[test]
        fn av1_pipeline_maps_to_av1_codec_name() {
            assert_eq!(pipeline_sdp_codec(Pipeline::Av1), Some("AV1"));
        }
    }

    #[cfg(all(test, feature = "h264"))]
    mod tier_bitrate_tests {
        use super::*;
        use desktop::encoders::BitrateBps;

        /// HiDPI on scales the per-tier bitrate by `scale_factor²` to track
        /// the pixel area growth (the encoder sees physical pixels in HiDPI
        /// mode). Result is capped at 30 Mbps so REMB clamping doesn't fight
        /// us under cellular bandwidth. Cap was 20 Mbps before the 2026-05-13
        /// quality uplift; raised to land in Sunshine/Moonlight territory for
        /// Retina captures. Area-scaling replaced a flat 2× on 2026-05-16
        /// after Retina/HiDPI motion was visibly smeared with the cursor
        /// trailing — see `260516-0136-hidpi-bitrate-area-scale` notes.
        #[test]
        fn tier_bitrate_off_matches_base_preset() {
            for sf in [1.0_f32, 1.5, 2.0] {
                assert_eq!(tier_bitrate(QualityTier::Low, sf, false).0, BitrateBps::LOW.0);
                assert_eq!(tier_bitrate(QualityTier::Med, sf, false).0, BitrateBps::MED.0);
                assert_eq!(tier_bitrate(QualityTier::High, sf, false).0, BitrateBps::HIGH.0);
            }
        }

        #[test]
        fn tier_bitrate_scales_by_area_on_hidpi() {
            // 2× Retina → 4× area → bitrate ×4 (capped at 30 Mbps).
            assert_eq!(
                tier_bitrate(QualityTier::Low, 2.0, true).0,
                (BitrateBps::LOW.0 as f64 * 4.0) as u32
            );
            assert_eq!(
                tier_bitrate(QualityTier::Med, 2.0, true).0,
                (BitrateBps::MED.0 as f64 * 4.0).min(30_000_000.0) as u32
            );
            // 12 Mbps × 4 = 48 Mbps → cap at 30 Mbps.
            assert_eq!(tier_bitrate(QualityTier::High, 2.0, true).0, 30_000_000);
        }

        #[test]
        fn tier_bitrate_scales_for_windows_fractional_dpi() {
            // Windows 125 % → scale_factor 1.25 → area 1.5625× the logical.
            let med_125 = tier_bitrate(QualityTier::Med, 1.25, true).0;
            assert_eq!(
                med_125,
                (BitrateBps::MED.0 as f64 * 1.5625).round() as u32
            );
            // Windows 150 % → 2.25×.
            let high_150 = tier_bitrate(QualityTier::High, 1.5, true).0;
            assert_eq!(
                high_150,
                (BitrateBps::HIGH.0 as f64 * 2.25).min(30_000_000.0).round() as u32
            );
        }

        #[test]
        fn tier_bitrate_clamps_scale_factor_below_one() {
            // Defensive: a stale or unset scale_factor mustn't shrink the
            // budget below the non-HiDPI preset.
            assert_eq!(
                tier_bitrate(QualityTier::Med, 0.5, true).0,
                BitrateBps::MED.0
            );
        }
    }
}

#[cfg(feature = "desktop")]
pub use inner::api_desktop_ws;
