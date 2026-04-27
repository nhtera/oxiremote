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

    use crate::auth::require_active_auth_with_device;
    use crate::desktop_service::DesktopService;
    use crate::desktop_ws_capture::{spawn_capture_pipeline, Sink};
    use crate::pipeline_selection::{
        choose as choose_pipeline, operator_preference, ClientCapabilities, Pipeline,
    };
    use crate::AppState;

    // ── Wire-format types ─────────────────────────────────────────────────────

    /// Signaling messages the agent receives from the client (WS text channel).
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum SignalIn {
        Offer { sdp: String },
        Ice { candidate: RTCIceCandidateInit },
        /// Phase 03: client announces decoder support. Agent uses this to
        /// pick JPEG vs H.264 pipeline. Presence of `webcodecs: true` and a
        /// codec in `codecs` matching our fmtp means H.264 is viable.
        CapabilitiesClient {
            #[serde(default)]
            codecs: Vec<String>,
            #[serde(default)]
            webcodecs: bool,
        },
        /// Phase 04: per-session capture settings. `hidpi=true` skips logical
        /// downscale so the encoder gets native physical pixels — costs ~4×
        /// pixel throughput on retina, paid for by a 2× bitrate multiplier
        /// in `tier_bitrate`. Toggling triggers a capture-pipeline restart
        /// because encoder dims are fixed once H.264 starts.
        Settings {
            #[serde(default)]
            hidpi: bool,
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
        /// Phase 03: server tells client which pipeline was selected and the
        /// per-tier bitrate presets. On H.264 mode `avcc_description` carries
        /// the base64-encoded `avcC` box built from the first IDR's SPS+PPS,
        /// ready for `VideoDecoder.configure({ description })`.
        Pipeline {
            mode: &'a str, // "h264" | "jpeg"
            codec: Option<&'a str>,
            tier_bitrates_kbps_low: u32,
            tier_bitrates_kbps_med: u32,
            tier_bitrates_kbps_high: u32,
            avcc_description_b64: Option<String>,
        },
        /// Capture pipeline exited mid-session (permission revoked, monitor
        /// unplugged, encoder error). Client uses this to fire its reconnect
        /// modal with the reason instead of staring at a frozen frame.
        CaptureEnded {
            reason: String,
        },
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
        /// Phase 04: mid-session HiDPI toggle. JPEG path applies live;
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
        // Register the default codec set so the H.264 track the server adds
        // matches a payload type in the client's offer. Without this, the
        // MediaEngine is empty and the answer rejects the video m-line with
        // `m=video 0 ... 0` (port 0 = disabled); the client's `ontrack`
        // never fires and the canvas stays black.
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
        // confirmed by `await_offer_with_caps` below.
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
        // Default to Med to match the web client's default dropdown value —
        // if the client's `quality` ctrl message is delayed we don't waste
        // cycles encoding native-retina frames no one asked for.
        #[cfg_attr(not(feature = "h264"), allow(unused_mut))]
        let (quality_tx, mut quality_rx) = watch::channel(QualityTier::Med);

        // ── Settings watch channel ────────────────────────────────────────────
        // Phase 04 HiDPI toggle. Initial value is filled in from the
        // pre-offer `Settings` signal (persisted in client localStorage); if
        // the client never sends one we stay in default-off (preserves
        // pre-Phase-04 behaviour).
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

        // ── Ctrl DC: parse and dispatch input events ──────────────────────────
        // The dispatch closure also needs to re-emit `Capabilities` over the
        // WS text channel whenever the tier changes so clients resize their
        // canvas to match the new encoder output grid.
        {
            let inj = injector.clone();
            let qtx = quality_tx.clone();
            let stx = settings_tx.clone();
            let caps_ws_tx = ws_out_tx.clone();
            ctrl_dc.on_message(Box::new(move |msg| {
                let inj = inj.clone();
                let qtx = qtx.clone();
                let stx = stx.clone();
                let caps_tx = caps_ws_tx.clone();
                Box::pin(async move {
                    let text = match std::str::from_utf8(&msg.data) {
                        Ok(t) => t,
                        Err(_) => return,
                    };
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

        // ── Offer / answer exchange ───────────────────────────────────────────
        // Split in two: read the offer + capabilities first, then complete
        // the SDP exchange. Between the two, when Pipeline::H264 we attach
        // a sendonly video track so the server's answer matches the client's
        // recvonly video m-line. The returned RTCRtpSender feeds
        // `spawn_rtcp_reader` for PLI + REMB feedback.
        #[cfg_attr(not(feature = "h264"), allow(unused_variables))]
        let (pipeline, offer_sdp, initial_hidpi) =
            await_offer_with_caps(&mut socket, &pc, &mut close_rx).await?;
        // Seed the settings watch with the pre-offer client preference so
        // the encoder starts at the user's persisted HiDPI mode without a
        // reconnect round-trip.
        let _ = settings_tx.send(initial_hidpi);

        // Build the H.264 sending track iff the Pipeline chose H.264 AND the
        // feature is compiled in. For the JPEG path `sending_track` is None
        // and `complete_answer` skips `pc.add_track`.
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

        let h264_sender = complete_answer(&pc, offer_sdp, &ws_out_tx, sending_track).await?;
        let _ = &h264_sender; // silence unused when feature = "h264" is off

        // Branch the transport pipeline now that the answer is on the wire.
        // H.264 uses an RTP track — no DC-open race, no JPEG tile encoder.
        #[cfg(feature = "h264")]
        if let (Pipeline::H264, Some(track), Some(sender)) =
            (pipeline, h264_track.as_ref(), h264_sender.as_ref())
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
                &quality_tx,
                &mut quality_rx,
                &settings_tx,
                &mut settings_rx,
                &mut close_rx,
            )
            .await;
            let _ = pc.close().await;
            return result;
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
        quality_tx: &watch::Sender<QualityTier>,
        quality_rx: &mut watch::Receiver<QualityTier>,
        settings_tx: &watch::Sender<bool>,
        settings_rx: &mut watch::Receiver<bool>,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        use base64::{engine::general_purpose::STANDARD as B64_STD, Engine as _};
        use desktop::capture::CaptureLoop;
        use desktop::encoders::BitrateBps;
        use desktop::resize_dims;

        let initial_tier = *quality_rx.borrow();
        let hidpi = initial_hidpi;
        let (out_w, out_h) = resize_dims(screen_w, screen_h, initial_tier, scale_factor, hidpi);

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
        let (bitrate_tx, bitrate_rx) = watch::channel(tier_bitrate(initial_tier, hidpi));
        let (pli_tx, pli_rx) = mpsc::channel::<()>(4);
        let (params_tx, params_rx) = tokio::sync::oneshot::channel();
        let (vp_shutdown_tx, vp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (rtcp_shutdown_tx, rtcp_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (cap_iframe_tx, cap_iframe_rx) = tokio::sync::oneshot::channel::<()>();
        // Force the first captured frame to be full — the encoder also forces
        // its first IDR, but firing here keeps behaviour symmetric with JPEG
        // where the starter tile-set is always complete.
        let _ = cap_iframe_tx.send(());

        // ── Spawn tasks ───────────────────────────────────────────────────────
        // Capture: resolution + HiDPI locked at session start (encoder dims are
        // fixed at init), but FPS tracks the live tier slider via `quality_rx`
        // so High delivers ~30 fps, Med ~15 fps, Low ~8 fps without restart.
        let cap_fps_rx = quality_tx.subscribe();
        tokio::task::spawn_blocking(move || {
            CaptureLoop::run_bgra(
                initial_tier,
                cap_fps_rx,
                bgra_tx,
                scale_factor,
                hidpi,
                Some(cap_iframe_rx),
            )
        });

        crate::video_pipeline::spawn_video_pipeline(crate::video_pipeline::VideoPipelineConfig {
            width: out_w,
            height: out_h,
            initial_bitrate: tier_bitrate(initial_tier, hidpi),
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
        });

        crate::video_pipeline::spawn_rtcp_reader(
            sender,
            pli_tx,
            bitrate_tx.clone(),
            rtcp_shutdown_rx,
        );

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
        let mut params_rx = Some(params_rx);
        loop {
            tokio::select! {
                biased;

                _ = &mut *close_rx => break,

                // First IDR's SPS/PPS — ship the avcC description once.
                res = wait_params(params_rx.as_mut()), if params_rx.is_some() => {
                    params_rx = None;
                    if let Some(params) = res {
                        let avcc = desktop::build_avcc(&params.sps, &params.pps);
                        let b64 = B64_STD.encode(&avcc);
                        let msg = SignalOut::Pipeline {
                            mode: "h264",
                            codec: Some("h264-baseline-3.1"),
                            tier_bitrates_kbps_low: BitrateBps::LOW.0 / 1_000,
                            tier_bitrates_kbps_med: BitrateBps::MED.0 / 1_000,
                            tier_bitrates_kbps_high: BitrateBps::HIGH.0 / 1_000,
                            avcc_description_b64: Some(b64),
                        };
                        if let Ok(txt) = serde_json::to_string(&msg) {
                            let _ = ws_out_tx.send(txt).await;
                        }
                        info!("h264: avcC description sent, decoder can configure");
                    }
                }

                // Push pending signaling (ICE + avcC) out to the WS client.
                Some(text) = ws_out_rx.recv() => {
                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                }

                // Tier change → new bitrate target. Dimensions cannot change
                // mid-session in H.264 mode (encoder size is fixed at init);
                // capabilities are re-emitted so the client stays in sync but
                // the encoder keeps its locked resolution.
                Ok(_) = quality_rx.changed() => {
                    let new_tier = *quality_rx.borrow();
                    let _ = bitrate_tx.send(tier_bitrate(new_tier, current_hidpi));
                    let (w, h) = resize_dims(
                        screen_w, screen_h, new_tier, scale_factor, current_hidpi,
                    );
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

                // Incoming WS text: late ICE + ctrl-over-ws fallback.
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
        Ok(())
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

    /// Map a quality tier (and HiDPI flag) to its H.264 bitrate preset. Keeps
    /// the bitrate table in one place so the `SignalOut::Pipeline` message
    /// and the encoder never drift apart.
    ///
    /// HiDPI doubles the encoder pixel count (~4× actually, but H.264
    /// compresses near-static UI well). The 2× bitrate multiplier captures
    /// most of the extra detail without overshooting REMB's typical ceiling.
    /// Capped at 20 Mbps so REMB clamping doesn't hammer the encoder back to
    /// its floor under cellular bandwidth.
    #[cfg(feature = "h264")]
    fn tier_bitrate(tier: QualityTier, hidpi: bool) -> desktop::encoders::BitrateBps {
        use desktop::encoders::BitrateBps;
        let base = match tier {
            QualityTier::High => BitrateBps::HIGH,
            QualityTier::Med => BitrateBps::MED,
            QualityTier::Low => BitrateBps::LOW,
        };
        if hidpi {
            BitrateBps((base.0.saturating_mul(2)).min(20_000_000))
        } else {
            base
        }
    }

    // ── Signaling: offer → answer ─────────────────────────────────────────────

    /// Read WS messages until an "offer" arrives. Does NOT process the offer
    /// — the caller may need to `pc.add_track(video_track)` after learning
    /// the Pipeline choice but before `create_answer` binds the SDP. ICE
    /// candidates that arrive before the offer are applied immediately.
    ///
    /// Also collects any pre-offer `Settings` so the encoder gets initialised
    /// with the user's persisted HiDPI preference on first frame — saves a
    /// reconnect round-trip when the client has toggled HiDPI on previously.
    ///
    /// Returns the chosen `Pipeline` (AND of operator env preference +
    /// client-advertised decoder capability), the offer SDP to feed into
    /// `complete_answer`, and the initial HiDPI flag (default `false`).
    async fn await_offer_with_caps(
        socket: &mut WebSocket,
        pc: &RTCPeerConnection,
        close_rx: &mut tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<(Pipeline, String, bool)> {
        let operator = operator_preference();
        let mut client_caps = ClientCapabilities::default();
        let mut initial_hidpi = false;

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
                            let chosen = choose_pipeline(operator, &client_caps);
                            info!(
                                operator = ?operator,
                                client_webcodecs = client_caps.webcodecs,
                                client_codec_count = client_caps.codecs.len(),
                                pipeline = chosen.wire_name(),
                                initial_hidpi,
                                "negotiation: offer received"
                            );
                            return Ok((chosen, sdp, initial_hidpi));
                        }
                        Ok(SignalIn::Ice { candidate }) => {
                            if let Err(e) = pc.add_ice_candidate(candidate).await {
                                warn!(error = %e, "pre-offer ICE candidate rejected");
                            }
                        }
                        Ok(SignalIn::CapabilitiesClient { codecs, webcodecs }) => {
                            client_caps.codecs = codecs;
                            client_caps.webcodecs = webcodecs;
                        }
                        Ok(SignalIn::Settings { hidpi }) => {
                            initial_hidpi = hidpi;
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
    ) -> anyhow::Result<Option<Arc<RTCRtpSender>>> {
        // Pre-bind the sendonly transceiver BEFORE set_remote_description so
        // `satisfy_type_and_direction` pairs it with the client's recvonly
        // m=video. `add_track()` after set_remote_description never reuses
        // the pending transceiver (webrtc-rs 0.11 only matches when the
        // track id equals the sender id — never true for fresh tracks), so
        // it spawns a second transceiver that doesn't map to any m-line and
        // the frames we write go nowhere. This change moves the binding
        // earlier so the single mid:0 m-line in the answer carries our RTP.
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
        pc.set_remote_description(RTCSessionDescription::offer(offer_sdp)?)
            .await?;
        let answer = pc.create_answer(None).await?;
        let sdp_out = answer.sdp.clone();
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
        // a signaling message (handled above).
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
    #[cfg(all(test, feature = "h264"))]
    mod tier_bitrate_tests {
        use super::*;
        use desktop::encoders::BitrateBps;

        /// HiDPI on doubles the per-tier bitrate, capped at 20 Mbps so REMB
        /// clamping doesn't fight us under cellular bandwidth.
        #[test]
        fn tier_bitrate_doubles_when_hidpi_on() {
            // Off → matches base preset.
            assert_eq!(tier_bitrate(QualityTier::Low, false).0, BitrateBps::LOW.0);
            assert_eq!(tier_bitrate(QualityTier::Med, false).0, BitrateBps::MED.0);
            assert_eq!(tier_bitrate(QualityTier::High, false).0, BitrateBps::HIGH.0);

            // On → 2× until the 20 Mbps cap.
            assert_eq!(
                tier_bitrate(QualityTier::Low, true).0,
                BitrateBps::LOW.0 * 2
            );
            assert_eq!(
                tier_bitrate(QualityTier::Med, true).0,
                BitrateBps::MED.0 * 2
            );
            // High = 12 Mbps × 2 = 24 Mbps → clamped to 20 Mbps.
            assert_eq!(tier_bitrate(QualityTier::High, true).0, 20_000_000);
        }
    }
}

#[cfg(feature = "desktop")]
pub use inner::api_desktop_ws;
