/// WebSocket handler for remote desktop streaming.
///
/// Handles:
/// - WebRTC signaling (offer/answer/ICE) over a JSON text channel on the same WS
/// - Two DataChannels: "desktop" (binary frames, unordered) and "ctrl" (input, ordered)
/// - 5-second fallback to WS-binary when DataChannel does not open in time
/// - Input injection via `InputInjector` wrapped in `Mutex` (injector is !Sync)
/// - Quality-tier changes restart the `CaptureLoop` via a `watch` channel
///
/// Wire formats: phase-04-remote-desktop-transport-and-ui.md
#[cfg(feature = "desktop")]
mod inner {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::ws::{Message, WebSocket};
    use axum::extract::{Path, State, WebSocketUpgrade};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::CookieJar;
    use bytes::Bytes;
    use desktop::input::{InputInjector, MouseBtn, PressAction};
    use desktop::{InputEvent, QualityTier};
    use serde::{Deserialize, Serialize};
    use tokio::sync::{mpsc, watch, Mutex};
    use tracing::{info, warn};
    use webrtc::api::APIBuilder;
    use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
    use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
    use webrtc::ice_transport::ice_server::RTCIceServer;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
    use webrtc::peer_connection::RTCPeerConnection;

    use crate::auth::require_active_auth_with_device;
    use crate::desktop_service::DesktopService;
    use crate::desktop_ws_capture::{spawn_capture_pipeline, Sink};
    use crate::AppState;

    // ── Wire-format types ─────────────────────────────────────────────────────

    /// Signaling messages the agent receives from the client (WS text channel).
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum SignalIn {
        Offer { sdp: String },
        Ice { candidate: RTCIceCandidateInit },
    }

    /// Signaling messages the agent sends to the client (WS text channel).
    #[derive(Debug, Serialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum SignalOut<'a> {
        Answer { sdp: &'a str },
        Ice { candidate: RTCIceCandidateInit },
        /// Sent once, before first binary frame, when 5-s DC-open timeout fires.
        Fallback,
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
        Quality {
            tier: String, // "high" | "med" | "low"
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
                WireInput::Quality { tier } => {
                    let t = parse_quality_tier(&tier)?;
                    Some(InputEvent::QualityChange { tier: t })
                }
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
        State(state): State<Arc<AppState>>,
        jar: CookieJar,
    ) -> impl IntoResponse {
        let Some((_session_id, session_device_id)) =
            require_active_auth_with_device(&state.db_path, &state.signing_key, &jar)
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

        let Some(ref svc) = state.desktop_service else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };

        // Single-viewer: evict any previous session for this device.
        let close_rx = svc.register(&device_id);
        let svc = Arc::clone(svc);

        ws.on_upgrade(move |socket| desktop_session(socket, device_id, state, svc, close_rx))
    }

    // ── Session entry ─────────────────────────────────────────────────────────

    async fn desktop_session(
        socket: WebSocket,
        device_id: String,
        state: Arc<AppState>,
        svc: Arc<DesktopService>,
        close_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        info!(device = %device_id, "desktop session opened");
        if let Err(e) = run_session(socket, &device_id, state, close_rx).await {
            warn!(device = %device_id, error = %e, "desktop session error");
        }
        svc.remove(&device_id);
        info!(device = %device_id, "desktop session closed");
    }

    async fn run_session(
        mut socket: WebSocket,
        device_id: &str,
        _state: Arc<AppState>,
        mut close_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        // ── Peer connection ───────────────────────────────────────────────────
        let api = APIBuilder::new().build();
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

        // ── DataChannels ──────────────────────────────────────────────────────
        // Both channels are externally negotiated: the client creates identical
        // DCs with the same stream ids, which pins the SCTP stream so send()
        // from either side reaches the other. Without `negotiated=Some(id)`
        // each side opens a fresh in-band stream and frames are sent into a
        // one-way void even though both peers report "DC open".
        //
        // "desktop": unordered + no retransmits — lowest latency for video tiles.
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

        // "ctrl": ordered — input events must not arrive out-of-order.
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

        // ── Quality watch channel ─────────────────────────────────────────────
        let (quality_tx, quality_rx) = watch::channel(QualityTier::High);

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

        // ── Ctrl DC: parse and dispatch input events ──────────────────────────
        {
            let inj = injector.clone();
            let qtx = quality_tx.clone();
            ctrl_dc.on_message(Box::new(move |msg| {
                let inj = inj.clone();
                let qtx = qtx.clone();
                Box::pin(async move {
                    let text = match std::str::from_utf8(&msg.data) {
                        Ok(t) => t,
                        Err(_) => return,
                    };
                    dispatch_input(text, &inj, screen_w, screen_h, &qtx).await;
                })
            }));
        }

        // ── DC-open signal (plain oneshot<()>) ────────────────────────────────
        // The on_open callback cannot pass the DC Arc; the session loop holds it.
        let (dc_open_tx, mut dc_open_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let tx = Arc::new(std::sync::Mutex::new(Some(dc_open_tx)));
            desktop_dc.on_open(Box::new(move || {
                let tx = Arc::clone(&tx);
                Box::pin(async move {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(s) = guard.take()
                    {
                        let _ = s.send(());
                    }
                })
            }));
        }

        // ── Offer / answer exchange ───────────────────────────────────────────
        negotiate(&mut socket, &pc, &ws_out_tx, &mut close_rx).await?;

        // ── 5-second fallback race ────────────────────────────────────────────
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
                    info!(device = %device_id, "desktop DC open — DataChannel path");
                    break Sink::DataChannel(Arc::clone(&desktop_dc));
                }

                _ = &mut timer => {
                    info!(device = %device_id, "desktop DC timeout — WS fallback");
                    if let Ok(msg) = serde_json::to_string(&SignalOut::Fallback) {
                        let _ = socket.send(Message::Text(msg.into())).await;
                    }
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
        // Read initial tier before moving quality_rx into the pipeline.
        let initial_tier = *quality_rx.borrow();
        spawn_capture_pipeline(initial_tier, sink, quality_rx, cap_shutdown_rx);

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
            &mut close_rx,
        )
        .await;

        // ── Teardown ──────────────────────────────────────────────────────────
        let _ = cap_shutdown_tx.send(());
        let _ = pc.close().await;
        Ok(())
    }

    // ── Signaling: offer → answer ─────────────────────────────────────────────

    /// Read WS messages until an "offer" arrives, complete the SDP exchange,
    /// and enqueue the answer on `ws_out_tx` (which `ws_loop` drains to the socket).
    /// ICE candidates that arrive before the offer are applied immediately.
    async fn negotiate(
        socket: &mut WebSocket,
        pc: &RTCPeerConnection,
        ws_out_tx: &mpsc::Sender<String>,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
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
                            pc.set_remote_description(RTCSessionDescription::offer(sdp)?).await?;
                            let answer = pc.create_answer(None).await?;
                            let sdp_out = answer.sdp.clone();
                            pc.set_local_description(answer).await?;
                            let msg = serde_json::to_string(&SignalOut::Answer { sdp: &sdp_out })?;
                            // Push answer into the outbound channel; ws_loop delivers it.
                            ws_out_tx.send(msg).await?;
                            return Ok(());
                        }
                        Ok(SignalIn::Ice { candidate }) => {
                            // ICE candidates may arrive before the offer in trickle-ICE.
                            if let Err(e) = pc.add_ice_candidate(candidate).await {
                                warn!(error = %e, "pre-offer ICE candidate rejected");
                            }
                        }
                        Err(e) => warn!(error = %e, "unknown signaling message — ignored"),
                    }
                }
            }
        }
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
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                biased;

                _ = &mut *close_rx => return,

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
                            on_incoming_text(&text, pc, injector, screen_w, screen_h, quality_tx).await;
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
    /// Could be a late ICE candidate or a ctrl event arriving over WS
    /// in fallback mode (when the DataChannel never opened).
    async fn on_incoming_text(
        text: &str,
        pc: &RTCPeerConnection,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        quality_tx: &watch::Sender<QualityTier>,
    ) {
        // ICE candidate?
        if let Ok(SignalIn::Ice { candidate }) = serde_json::from_str(text) {
            if let Err(e) = pc.add_ice_candidate(candidate).await {
                warn!(error = %e, "post-offer ICE candidate rejected");
            }
            return;
        }

        // Ctrl / input event (fallback mode — DC never opened)?
        // dispatch_input parses internally; only call it when the text is not
        // a signaling message (ICE already handled above).
        dispatch_input(text, injector, screen_w, screen_h, quality_tx).await;
    }

    /// Parse a ctrl-channel message and dispatch to InputInjector or quality_tx.
    async fn dispatch_input(
        text: &str,
        injector: &Option<Arc<Mutex<InputInjector>>>,
        screen_w: u32,
        screen_h: u32,
        quality_tx: &watch::Sender<QualityTier>,
    ) {
        let wire: WireInput = match serde_json::from_str(text) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "ctrl: invalid input JSON");
                return;
            }
        };

        // Quality changes are handled before converting to InputEvent.
        if let WireInput::Quality { ref tier } = wire
            && let Some(t) = parse_quality_tier(tier)
        {
            info!(tier = ?t, "quality change requested");
            let _ = quality_tx.send(t);
            return;
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
}

#[cfg(feature = "desktop")]
pub use inner::api_desktop_ws;
