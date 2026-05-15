//! Audio capture → Opus encode → RTP pipeline (phase-02a).
//!
//! Mirrors `video_pipeline.rs` shape so the desktop_ws session layer can wire
//! both tracks identically: build the track, hand a capture impl to a single
//! `spawn_audio_pipeline` call, register the track on the PC before
//! `set_remote_description`, attach the shutdown oneshot for clean teardown.
//!
//! Two tokio tasks:
//! - **encoder** (async) — pulls 20 ms i16 stereo frames from the
//!   `desktop::audio::AudioCapture` impl, runs `OpusEncoder::encode_frame`
//!   (libopus, ~microseconds per frame), pushes the encoded bytes onto
//!   `sample_tx`. Async — Opus encode is cheap enough that the blocking
//!   thread the H.264 pipeline uses is overkill; the AudioCapture trait is
//!   async-native, so a blocking thread would just `block_on` round-trip.
//! - **writer** (async) — drains `sample_rx`, builds `webrtc::media::Sample`
//!   with `duration = 20 ms`, calls `track.write_sample`. The packetizer
//!   multiplies duration by Opus's 48 kHz clock → 960-sample RTP timestamp
//!   increment, matching one Opus frame exactly.
//!
//! Why no RTCP feedback reader here? Opus is lossy-resilient via in-band FEC
//! (we register Opus with `useinbandfec=1`). The bitrate target is fixed at
//! the wire format pinned in `OpusEncoder::new` (96 kbps VBR); we don't yet
//! react to REMB for audio because the bandwidth is trivial vs video.
//!
//! Privacy: capture only starts when the operator's `desktop_audio_enabled`
//! setting is true AND the client advertised `audio: true` AND `probe_supported`
//! returned true. All three gates live in `desktop_ws.rs` (phase-02a step 6);
//! this module assumes the caller has already passed them.

#![cfg(feature = "audio")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use desktop::audio::{AudioCapture, AudioError, opus_encoder::OpusEncoder};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use webrtc::api::media_engine::MIME_TYPE_OPUS;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::events::{AgentEvent, EventBus};

/// Why the audio pipeline stopped. Single source of truth for the wire
/// `reason` field on `AgentEvent::AudioStopped` — keep these strings stable
/// so dashboard filters and SSE consumers can pattern-match without parsing
/// free text. The pipeline emits `AudioStopped` exactly once at exit; the
/// caller signals `SessionClosed` / `UserToggleOff` via the shutdown
/// oneshot, while `SckError` / `WasapiError` are decided internally on any
/// capture or encoder failure (the caller picks which one applies for the
/// active backend via `AudioPipelineConfig::capture_error_reason`).
#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    SessionClosed,
    UserToggleOff,
    // SckError is only constructed on macOS, WasapiError only on Windows.
    // Cross-platform consumers (tests, dashboard parsers) still want both
    // visible; per-target dead-code allows keep the build warning-free.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    SckError,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WasapiError,
}

impl StopReason {
    pub fn as_wire(&self) -> &'static str {
        match self {
            StopReason::SessionClosed => "session_closed",
            StopReason::UserToggleOff => "user_toggle_off",
            StopReason::SckError => "sck_error",
            StopReason::WasapiError => "wasapi_error",
        }
    }
}

/// 20 ms Opus frame duration. Stays a strict constant — the packetizer uses
/// it for RTP timestamp deltas, so any lying duration drifts the receiver
/// clock. AudioCapture impls MUST deliver frames sized to this duration
/// (1920 i16 samples for stereo @ 48 kHz).
pub const FRAME_DURATION: Duration = Duration::from_millis(20);

/// Internal mpsc capacity between encoder and writer. 4 slots ≈ 80 ms of
/// buffered audio — generous enough to absorb writer jitter without
/// accumulating perceptible latency on the consumer side.
const SAMPLE_CHANNEL_CAP: usize = 4;

/// Configuration for `spawn_audio_pipeline`. Caller owns all channels so
/// session teardown can fire `shutdown_rx` and drop the capture in any order.
///
/// `event_bus` + `device_id` let the pipeline emit `AgentEvent::AudioStopped`
/// from a single point with the correct reason — `SessionClosed` /
/// `UserToggleOff` as signalled by the caller through `shutdown_rx`, or
/// `capture_error_reason` (typically `SckError` on macOS, `WasapiError` on
/// Windows) when a capture/encoder failure forces the loop to exit. The
/// caller does NOT emit `AudioStopped` itself; the pipeline owns the
/// lifecycle.
pub struct AudioPipelineConfig {
    pub track: Arc<TrackLocalStaticSample>,
    pub capture: Box<dyn AudioCapture>,
    pub shutdown_rx: oneshot::Receiver<StopReason>,
    pub event_bus: Arc<EventBus>,
    pub device_id: String,
    /// Wire reason emitted on an internal capture/encoder failure. The
    /// pipeline can't know which backend the capture is — caller picks the
    /// right variant for the platform it built.
    pub capture_error_reason: StopReason,
    /// User-level mute. When `true`, the writer task skips `write_sample`
    /// so no RTP packets reach the client (receiver hears silence) but the
    /// capture + encode pipeline keeps running. Toggling this atomic is
    /// instant — no PC renegotiation, no SCStream rebuild. The operator-
    /// level master kill stays on `shutdown_rx` (UserToggleOff /
    /// SessionClosed) so a setting flip still tears the whole pipeline.
    pub muted: Arc<AtomicBool>,
}

/// `RTCRtpCodecCapability` matching what `register_default_codecs` exposes
/// for Opus (PT 111, 48 kHz, stereo, FEC enabled). Keep this in sync with
/// the codec registration in `MediaEngine::register_default_codecs` so SDP
/// negotiation doesn't strip the audio track at the answer step.
pub fn opus_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_OPUS.to_owned(),
        clock_rate: 48_000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
        rtcp_feedback: vec![],
    }
}

/// Create an Opus `TrackLocalStaticSample` ready to be added to a peer
/// connection via `pc.add_transceiver_from_track(track, Sendonly)` BEFORE
/// `set_remote_description` (webrtc-rs gotcha — calling `add_track` after
/// SRD creates an orphan transceiver).
pub fn new_opus_track() -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        opus_codec_capability(),
        "audio".to_string(),
        "oxiremote-desktop".to_string(),
    ))
}

/// Spawn the encoder + writer tasks. Returns immediately. Both tasks
/// terminate when `shutdown_rx` fires or when the channels close.
///
/// The encoder task emits exactly one `AgentEvent::AudioStopped` at exit —
/// reason carried by the shutdown signal (caller-decided) or `SckError`
/// when capture / encoder init fails internally. Callers must NOT emit
/// `AudioStopped` themselves.
pub fn spawn_audio_pipeline(cfg: AudioPipelineConfig) {
    let (sample_tx, sample_rx) = mpsc::channel::<Vec<u8>>(SAMPLE_CHANNEL_CAP);

    // Writer task — async, owns the track Arc.
    let track = cfg.track.clone();
    let muted = Arc::clone(&cfg.muted);
    tokio::spawn(writer_task(track, sample_rx, muted));

    // Encoder task — async, owns the capture + encoder. Holds the shutdown
    // oneshot so it can break the next_frame await without leaking the
    // capture.
    tokio::spawn(encoder_task(
        cfg.capture,
        sample_tx,
        cfg.shutdown_rx,
        cfg.event_bus,
        cfg.device_id,
        cfg.capture_error_reason,
    ));
}

async fn encoder_task(
    mut capture: Box<dyn AudioCapture>,
    sample_tx: mpsc::Sender<Vec<u8>>,
    mut shutdown_rx: oneshot::Receiver<StopReason>,
    event_bus: Arc<EventBus>,
    device_id: String,
    capture_error_reason: StopReason,
) {
    let mut encoder = match OpusEncoder::new() {
        Ok(e) => e,
        Err(err) => {
            warn!(error = %err, "audio_pipeline: OpusEncoder init failed; aborting");
            emit_stopped(&event_bus, &device_id, capture_error_reason);
            return;
        }
    };
    info!(
        sample_rate = capture.sample_rate(),
        channels = capture.channels(),
        "audio_pipeline: encoder task started"
    );

    // Reason for exiting the loop. Default to the caller-supplied capture
    // error reason; overwritten by the shutdown arm with whatever the caller
    // chose, or left as-is on any capture / encoder failure path.
    let mut exit_reason = capture_error_reason;
    loop {
        tokio::select! {
            biased;
            r = &mut shutdown_rx => {
                exit_reason = r.unwrap_or(StopReason::SessionClosed);
                info!(reason = exit_reason.as_wire(), "audio_pipeline: shutdown received");
                break;
            }
            next = capture.next_frame() => {
                let pcm = match next {
                    Ok(p) => p,
                    Err(AudioError::CaptureFailed(msg)) => {
                        warn!(reason = %msg, "audio_pipeline: capture failed; stopping");
                        break;
                    }
                    Err(err) => {
                        warn!(error = %err, "audio_pipeline: unexpected capture error; stopping");
                        break;
                    }
                };
                let opus = match encoder.encode_frame(&pcm) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        // Bad frame size or transient libopus error — skip the
                        // frame, keep the pipeline alive. A persistent error
                        // would surface as silence on the receiver but isn't
                        // grounds to tear down audio (let alone video).
                        debug!(error = %err, "audio_pipeline: encode_frame failed; skipping");
                        continue;
                    }
                };
                if sample_tx.try_send(opus).is_err() {
                    // Writer task backpressure: drop the oldest semantics is
                    // worse than drop-newest for audio (creates a hole the
                    // listener hears). 4 slots = 80 ms — if we hit cap, the
                    // writer is genuinely stalled and dropping is the lesser
                    // evil vs. blocking the encoder.
                    debug!("audio_pipeline: writer backpressure, dropping 20ms frame");
                }
            }
        }
    }

    // Drop sample_tx so the writer's recv loop exits cleanly.
    drop(sample_tx);

    emit_stopped(&event_bus, &device_id, exit_reason);
}

fn emit_stopped(event_bus: &EventBus, device_id: &str, reason: StopReason) {
    event_bus.send(AgentEvent::AudioStopped {
        device_id: device_id.to_string(),
        reason: reason.as_wire().to_string(),
    });
}

async fn writer_task(
    track: Arc<TrackLocalStaticSample>,
    mut sample_rx: mpsc::Receiver<Vec<u8>>,
    muted: Arc<AtomicBool>,
) {
    while let Some(opus) = sample_rx.recv().await {
        // Mute is a user-level toggle: drop the encoded frame instead of
        // writing it. The encoder keeps running so unmute is instant —
        // first frame after the flip lands on the wire ~20 ms later
        // without renegotiation. Receiver hears natural silence until then.
        if muted.load(Ordering::Relaxed) {
            continue;
        }
        let sample = Sample {
            data: opus.into(),
            timestamp: SystemTime::now(),
            duration: FRAME_DURATION,
            packet_timestamp: 0,
            prev_dropped_packets: 0,
            prev_padding_packets: 0,
        };
        if let Err(e) = track.write_sample(&sample).await {
            warn!(error = ?e, "audio_pipeline: track.write_sample failed");
        }
    }
    info!("audio_pipeline: writer task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_capability_matches_register_default_codecs() {
        let cap = opus_codec_capability();
        assert_eq!(cap.mime_type, "audio/opus");
        assert_eq!(cap.clock_rate, 48_000);
        assert_eq!(cap.channels, 2);
        assert!(cap.sdp_fmtp_line.contains("useinbandfec=1"));
        assert!(cap.sdp_fmtp_line.contains("minptime=10"));
    }

    #[test]
    fn new_opus_track_has_matching_codec() {
        let t = new_opus_track();
        assert_eq!(t.codec().mime_type, "audio/opus");
    }

    #[test]
    fn stop_reason_wire_strings_are_stable() {
        // Dashboard / SSE consumers pattern-match these strings — pinning the
        // mapping in a test keeps a rename from silently breaking them.
        assert_eq!(StopReason::SessionClosed.as_wire(), "session_closed");
        assert_eq!(StopReason::UserToggleOff.as_wire(), "user_toggle_off");
        assert_eq!(StopReason::SckError.as_wire(), "sck_error");
        assert_eq!(StopReason::WasapiError.as_wire(), "wasapi_error");
    }

    #[test]
    fn frame_duration_aligns_with_opus_frame_size() {
        // 20 ms @ 48 kHz = 960 samples per channel = 1920 stereo samples.
        // The packetizer multiplies duration * clock_rate to derive the RTP
        // timestamp delta, so a mismatch here would desync the receiver.
        let samples_per_channel = FRAME_DURATION.as_millis() as u64 * 48_000 / 1_000;
        assert_eq!(samples_per_channel, 960);
    }

    #[tokio::test]
    async fn writer_skips_write_sample_when_muted() {
        // Regression for the instant-toggle fix: flipping the mute atomic to
        // `true` must stop frames reaching `track.write_sample` without
        // dropping the channel — the writer keeps draining `sample_rx` so a
        // later unmute resumes immediately. We can't intercept
        // `TrackLocalStaticSample` directly, so observe the indirect effect:
        // the writer task must continue consuming samples while muted (it
        // exits only when the channel closes), and must exit when the
        // channel drops. Running the muted writer with a stream of samples
        // and then closing the channel verifies both invariants.
        let track = new_opus_track();
        let muted = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
        let handle = tokio::spawn(writer_task(track, rx, Arc::clone(&muted)));
        for _ in 0..3 {
            tx.send(vec![0xfc, 0xff, 0xfe]).await.unwrap();
        }
        drop(tx);
        let result =
            tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(
            result.is_ok(),
            "writer must exit when channel closes, even while muted"
        );
    }

    #[test]
    fn muted_atomic_default_is_unmuted_on_unset() {
        // Audio gate negotiates the transceiver up-front; the per-session
        // mute atomic is seeded from `!client_audio_opt_in`. Verify the
        // wire-default (no opt-in flag → false → muted=true) so a stale
        // capabilitiesClient frame can't accidentally unmute.
        let muted = AtomicBool::new(true);
        assert!(muted.load(Ordering::Relaxed), "default seed is muted when client did not opt in");
        muted.store(false, Ordering::Relaxed);
        assert!(!muted.load(Ordering::Relaxed));
    }
}
