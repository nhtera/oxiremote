//! H.264 capture → encode → RTP pipeline.
//!
//! Wires a `desktop::H264Encoder` into a webrtc-rs `TrackLocalStaticSample`
//! behind two tokio tasks:
//! - **encoder** (blocking) — pulls BGRA frames from `bgra_rx`, encodes to
//!   Annex-B, pushes `EncodedFrame` onto `sample_tx`.
//! - **writer** (async) — drains `sample_rx`, builds `webrtc::media::Sample`,
//!   calls `track.write_sample` so RTP packets go out.
//!
//! The encoder is trait-dispatched (VT on macOS, OpenH264 elsewhere) so the
//! pipeline code has zero FFI — all the unsafe lives in
//! `desktop::encoders::videotoolbox_encoder`.
//!
//! Feedback is bidirectional: PLI packets arriving on the RTCP read loop
//! force the next encoded frame to be an IDR (recovery), and REMB packets
//! drive a `watch<BitrateBps>` the encoder polls between frames.

#![cfg(feature = "h264")]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use desktop::encoders::{BitrateBps, EncodedFrame, H264Encoder, ParameterSets};
use desktop::{QualityTier, RawBgraFrame};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::desktop_abr::AbrObservation;

/// Default duration for the very first sample (no prior frame to measure
/// from). 33 ms ≈ 30 fps — a reasonable seed at any tier; subsequent
/// samples carry the true elapsed wall-clock interval.
const FIRST_FRAME_DURATION_MS: u64 = 33;

/// Minimum spacing between PLI-driven IDRs. Caps the death-spiral on
/// congested paths: when the decoder sends a PLI on every dropped frame,
/// honoring every PLI floods the budget with keyframes (each ~5-10× a
/// P-frame), exhausts the REMB-clamped bitrate, and starves new content —
/// which loses more packets, which drives more PLIs. 1 s is the
/// libwebrtc default and matches what Chrome's own sender does
/// internally. Recovery latency upper bound is therefore ~1 RTT + 1 s
/// in the worst case; observed quality wins outweigh the latency cost.
///
/// Natural keyframes (the encoder's `MaxKeyFrameInterval = 60 frames`
/// pacing, and session-start `force_iframe`) are NOT rate-limited — only
/// PLI-triggered forces.
const PLI_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Decide whether a pending PLI should force the next frame to IDR. Pure
/// function so the throttling decision is unit-testable without driving
/// the whole encoder loop. Returns `true` on a cold start (no IDR yet) or
/// when the last IDR is older than `PLI_MIN_INTERVAL`.
fn pli_force_allowed(last_idr_at: Option<Instant>, now: Instant) -> bool {
    match last_idr_at {
        None => true,
        Some(t) => now.duration_since(t) >= PLI_MIN_INTERVAL,
    }
}

/// Alias for the canonical capture-layer type. Kept as a re-export so the
/// transport-layer API (`VideoPipelineConfig`) stays self-contained.
pub type BgraFrame = RawBgraFrame;

/// Configuration for `spawn_video_pipeline`. All channels are owned by the
/// caller so they can tear down cleanly on session close.
pub struct VideoPipelineConfig {
    pub width: u32,
    pub height: u32,
    pub initial_bitrate: BitrateBps,
    /// Per-frame quality floor for VideoToolbox (H.264 QP 0-51). `Some(n)`
    /// caps QP at `n` — encoder must spend bits to keep QP ≤ `n` or drop
    /// the frame. `None` disables the cap (bandwidth-first behavior).
    /// Computed from the operator's initial tier in `desktop_ws.rs`.
    /// Session-lifetime constant; mid-session tier changes do NOT update
    /// the cap (VT requires session rebuild for that — out of scope).
    pub initial_max_qp: Option<u32>,
    pub track: Arc<TrackLocalStaticSample>,
    pub bgra_rx: mpsc::Receiver<BgraFrame>,
    pub bitrate_rx: watch::Receiver<BitrateBps>,
    /// Live tier signal — drives the writer's send-pacing cadence so RTP
    /// arrival at the browser is steady at the user's selected fps. Without
    /// this the burst-y SCK arrival pattern fattens Chrome's jitter buffer.
    pub fps_rx: watch::Receiver<QualityTier>,
    pub shutdown_rx: oneshot::Receiver<()>,
    /// PLI arrivals from the RTCP read loop. Each received `()` forces the
    /// *next* encoded frame to be an IDR so the client can recover from a
    /// decode loss. Multiple PLIs coalesce — one queued signal is enough.
    pub pli_rx: mpsc::Receiver<()>,
    /// Receives the cached SPS/PPS the first time the encoder produces an
    /// IDR. Used by the session layer to build the `avcC` config blob sent
    /// over the ctrl DC before the first RTP frame. Only one send per
    /// pipeline lifetime — treat subsequent calls as no-op.
    pub params_tx: oneshot::Sender<ParameterSets>,
    /// Phase-03 ABR observation channel. Encoder task pushes `Encode`
    /// observations per frame (timing + queue depth + current bitrate);
    /// `spawn_rtcp_reader` pushes `Network` observations per RR/REMB. The
    /// controller + stats SSE subscribe via `tx.subscribe()`. Slow
    /// consumers see `Lagged` and skip ahead — never block producers.
    pub abr_tx: broadcast::Sender<AbrObservation>,
}

/// Build the `RTCRtpCodecCapability` we expose in the SDP offer — H.264
/// **High** profile, Level 5.0, 90 kHz clock, packetization-mode=1.
///
/// `profile-level-id=640032` decodes as: profile_idc=0x64 (100, High),
/// profile-iop=0x00 (no constraint flags — High doesn't carry the
/// baseline-compatibility bits), level_idc=0x32 (Level 5.0,
/// max ~2560×1920 / 36 Mbps). Bumped from `42e032` (Constrained Baseline)
/// on 2026-05-13 — plan `260513-0009-h264-quality-uplift-vt-high-profile`.
/// All target browsers (iPad Safari 17+, Chrome desktop/Android,
/// Firefox 130+) hardware-decode H.264 High 4:2:0 over WebRTC in 2026.
///
/// The encoder runs at `AutoLevel` so VT emits the minimum level the
/// actual resolution requires; advertising 5.0 here lets the SDP
/// negotiate that envelope without false-rejection from browsers that
/// strictly check `profile-level-id`.
pub fn h264_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H264.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line:
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=640032".to_string(),
        rtcp_feedback: vec![],
    }
}

/// Create an H.264 `TrackLocalStaticSample` ready to be added to a peer
/// connection via `pc.add_track(track)`. The returned Arc is shared between
/// the pipeline task (writes samples) and the peer connection (sends RTP).
pub fn new_h264_track() -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        h264_codec_capability(),
        "video".to_string(),
        "oxiremote-desktop".to_string(),
    ))
}

/// Construct the correct encoder backend for the current platform. Returns
/// a heap-allocated trait object so the pipeline is oblivious to which
/// backend is driving.
///
/// `max_qp` is the per-frame quality floor for VT (H.264 QP 0-51). Plumbed
/// from the operator's initial tier — typically HIGH=23, MED=28, LOW=None.
/// OpenH264 (software fallback) ignores this — its high-level binding
/// doesn't expose ISVCEncoder::SetOption today.
fn build_encoder(
    width: u32,
    height: u32,
    bitrate: BitrateBps,
    max_qp: Option<u32>,
) -> Result<Box<dyn H264Encoder>> {
    #[cfg(target_os = "macos")]
    {
        match desktop::encoders::videotoolbox_encoder::VideoToolboxEncoder::new(
            width, height, bitrate, max_qp,
        ) {
            Ok(enc) => {
                info!(max_qp = ?max_qp, "video_pipeline: using VideoToolbox (hardware)");
                return Ok(Box::new(enc));
            }
            Err(e) => warn!(error = %e, "VideoToolbox init failed, falling back to OpenH264"),
        }
    }
    let _ = max_qp; // OpenH264 has no equivalent knob today.
    let enc = desktop::encoders::openh264_encoder::OpenH264Encoder::new(width, height, bitrate)?;
    info!("video_pipeline: using OpenH264 (software)");
    Ok(Box::new(enc))
}

/// Spawn the encode + write tasks. Returns immediately. Both tasks terminate
/// when `shutdown_rx` fires or when the channels close.
pub fn spawn_video_pipeline(mut cfg: VideoPipelineConfig) {
    // Capacity 1 — combined with the writer's drain-to-latest below this
    // gives drop-oldest semantics: encoder always pushes the freshest sample,
    // writer always pulls the freshest sample, and any frame stuck queueing
    // behind a slow `track.write_sample()` is replaced rather than aged.
    // Lowers worst-case glass-to-glass by one frame interval.
    let (sample_tx, sample_rx) = mpsc::channel::<EncodedFrame>(1);

    // Writer task — async, owns the track Arc.
    let track = cfg.track.clone();
    let writer_fps_rx = cfg.fps_rx.clone();
    tokio::spawn(writer_task(track, sample_rx, writer_fps_rx));

    // Encoder task — blocking, owns the encoder + params_tx oneshot.
    std::thread::spawn(move || {
        let mut encoder = match build_encoder(cfg.width, cfg.height, cfg.initial_bitrate, cfg.initial_max_qp) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "video_pipeline: encoder build failed, pipeline stopping");
                return;
            }
        };
        let mut params_tx = Some(cfg.params_tx);
        let mut last_bitrate = cfg.initial_bitrate.0;
        let mut pli_pending = false;
        // Tracks the most recent keyframe wall-clock time from ANY source
        // (PLI force, session-start force, encoder's natural keyframe
        // interval). Drives `pli_force_allowed`. None until the first IDR
        // lands; the first force-IDR is always honored.
        let mut last_idr_at: Option<Instant> = None;
        info!("video_pipeline: encoder task started");

        loop {
            // Poll shutdown cheaply each loop iteration.
            if cfg.shutdown_rx.try_recv().is_ok() {
                info!("video_pipeline: shutdown received");
                break;
            }

            // Drain any PLIs queued since the last frame. Multiple PLIs
            // coalesce to a single force-IDR: clients that send several in
            // quick succession still only need one recovery keyframe.
            while cfg.pli_rx.try_recv().is_ok() {
                pli_pending = true;
            }

            // Pick up bitrate changes without blocking — watch::Receiver
            // surfaces the latest value via `borrow`.
            if cfg.bitrate_rx.has_changed().unwrap_or(false) {
                let new_bps = cfg.bitrate_rx.borrow_and_update().0;
                if new_bps != last_bitrate {
                    if let Err(e) = encoder.set_bitrate(BitrateBps(new_bps)) {
                        warn!(error = %e, "encoder set_bitrate failed");
                    } else {
                        last_bitrate = new_bps;
                    }
                }
            }

            let Some(mut frame) = cfg.bgra_rx.blocking_recv() else {
                // bgra_rx closed — capture loop shut down.
                break;
            };
            // Drain any extra frames the capture loop produced while we were
            // encoding. Keep only the freshest — encoding a stale frame just
            // adds latency for no quality benefit. force_idr is OR-ed across
            // all drained frames so a queued PLI-driven IDR isn't lost.
            let mut drained_force_idr = frame.force_idr;
            while let Ok(newer) = cfg.bgra_rx.try_recv() {
                drained_force_idr |= newer.force_idr;
                frame = newer;
            }

            // Snapshot bgra queue depth BEFORE encode — `len()` reflects
            // backlog left after the drain loop above (so a sustained
            // non-zero indicates capture out-pacing the encoder).
            let bgra_queue_depth = cfg.bgra_rx.len();

            // Decide whether to honor a pending PLI. `drained_force_idr`
            // comes from the capture loop's session-start `force_iframe_rx`
            // (oneshot, fires once); always honored. `pli_pending` is from
            // the RTCP reader; honored only after `PLI_MIN_INTERVAL` since
            // the last keyframe. Unhonored PLIs stay queued — they'll fire
            // on the first eligible frame instead of being silently
            // dropped, so a single recovery request can't be lost.
            let now_pli = Instant::now();
            let pli_honored = pli_pending && pli_force_allowed(last_idr_at, now_pli);
            let force_idr = drained_force_idr || pli_honored;
            let encode_started = Instant::now();
            let encoded = match encoder.encode(
                &frame.bytes,
                frame.width,
                frame.height,
                force_idr,
            ) {
                Ok(Some(f)) => {
                    if pli_honored {
                        pli_pending = false;
                    }
                    f
                }
                Ok(None) => continue, // encoder chose to skip this frame
                Err(e) => {
                    warn!(error = %e, "encoder.encode failed");
                    continue;
                }
            };
            let encode_ms = encode_started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            // Update the throttle anchor on any keyframe — including the
            // encoder's spontaneous IDRs from `MaxKeyFrameInterval`, so the
            // window doesn't restart from a force-IDR alone.
            if encoded.is_keyframe {
                last_idr_at = Some(Instant::now());
            }

            // First-keyframe → forward SPS/PPS so the session layer can
            // build the avcC description blob for WebCodecs.
            if encoded.is_keyframe
                && let (Some(tx), Some(p)) = (params_tx.take(), encoder.parameter_sets())
            {
                let _ = tx.send(p);
            }

            // Phase-03: emit per-frame ABR observation. `sample_tx` cap=1 so
            // queue depth ∈ {0,1}; the difference (cap − permits) is the live
            // fill. Send is fire-and-forget — broadcast returns Err only when
            // there are no receivers, which is the steady state until the
            // controller/SSE attach.
            let is_keyframe = encoded.is_keyframe;
            let sample_queue_depth = sample_tx
                .max_capacity()
                .saturating_sub(sample_tx.capacity());
            let _ = cfg.abr_tx.send(AbrObservation::Encode {
                encode_ms,
                is_keyframe,
                bgra_queue_depth,
                sample_queue_depth,
                current_bitrate_kbps: last_bitrate / 1_000,
            });

            // Bounded channel: drop the oldest if writer is behind.
            if sample_tx.try_send(encoded).is_err() {
                debug!("video_pipeline: writer backpressure, dropping frame");
            }
        }

        // Drain sample_tx so the writer can exit cleanly.
        drop(sample_tx);
    });
}

/// Consume `EncodedFrame`s and hand them to the track as `webrtc::media::Sample`.
async fn writer_task(
    track: Arc<TrackLocalStaticSample>,
    mut sample_rx: mpsc::Receiver<EncodedFrame>,
    fps_rx: watch::Receiver<QualityTier>,
) {
    use std::time::Instant;
    let mut last_send: Option<Instant> = None;
    while let Some(mut frame) = sample_rx.recv().await {
        // Drain any encoded samples queued behind this one — drop-oldest
        // semantics on top of a bounded mpsc. write_sample on a stale frame
        // adds latency without benefit; the decoder resyncs on the next
        // keyframe (≤ 2 s by `MaxKeyFrameInterval`, sooner on PLI).
        while let Ok(newer) = sample_rx.try_recv() {
            frame = newer;
        }

        // PACED SEND: hold the sample until at least `target_interval` has
        // elapsed since the previous write. SCK can deliver frames in bursts
        // (4 frames in 50 ms then idle for 200 ms is normal for a static
        // screen); shipping them as they arrive fattens Chrome's jitter
        // buffer to absorb the unevenness. Forcing a steady cadence at the
        // user's tier rate lets the browser play with near-zero buffering.
        let target_interval = desktop::frame_interval(*fps_rx.borrow());
        if let Some(prev) = last_send {
            let elapsed = prev.elapsed();
            if elapsed < target_interval {
                tokio::time::sleep(target_interval - elapsed).await;
            }
        }
        // Re-drain after the pacing sleep — a fresher frame may have arrived
        // while we waited; sending it instead of the now-stale one shaves
        // up to one tier-interval of latency.
        while let Ok(newer) = sample_rx.try_recv() {
            frame = newer;
        }

        // Measure ACTUAL wall-clock interval since the previous sample for
        // the RTP timestamp delta. The packetizer multiplies this by the
        // 90 kHz clock rate, so a lying duration drifts the receiver clock
        // and balloons the jitter buffer. Clamp [1 ms, 200 ms] so a stalled
        // capture loop can't emit a duration that confuses the receiver.
        let now = Instant::now();
        let duration = match last_send {
            Some(prev) => now
                .duration_since(prev)
                .clamp(Duration::from_millis(1), Duration::from_millis(200)),
            None => Duration::from_millis(FIRST_FRAME_DURATION_MS),
        };
        last_send = Some(now);
        let sample = Sample {
            data: frame.annexb,
            timestamp: SystemTime::now(),
            duration,
            packet_timestamp: 0,
            prev_dropped_packets: 0,
            prev_padding_packets: 0,
        };
        if let Err(e) = track.write_sample(&sample).await {
            warn!(error = ?e, "track.write_sample failed");
        }
    }
    info!("video_pipeline: writer task exiting");
}

// ─── RTCP feedback reader ────────────────────────────────────────────────────
//
// webrtc-rs 0.11 exposes no `on_pli` / `on_remb` callbacks — per research
// memo A, both arrive via `sender.read_rtcp()` returning a `Vec<Box<dyn
// rtcp::packet::Packet>>`. We own the read loop ourselves, downcast each
// packet, and fan out to two channels:
// - `pli_tx`   — one empty signal per PLI; encoder task must force next IDR.
// - `remb_tx`  — latest REMB bitrate estimate in bps; encoder task uses as
//                the target for `set_bitrate`. We don't debounce; encoder
//                applies the current value at its own cadence.

/// Spawn the RTCP read loop. The task terminates when `shutdown_rx` fires,
/// when the sender closes, or when `read_rtcp` returns a persistent error.
///
/// `loopback_bypass=true` — emitted by the SPA when its origin is
/// localhost / 127.* / ::1 — disables the REMB → encoder-bitrate path. The
/// encoder stays at the tier bitrate; REMB observations still reach the
/// stats SSE so the overlay shows the (broken) Chrome BWE estimate. This
/// matches Parsec's BUD2 / Rustdesk's TLS-direct posture: skip generic
/// BWE on local paths because GCC misreads CPU-contention scheduling
/// jitter as network congestion and collapses to ~5 kbps.
pub fn spawn_rtcp_reader(
    sender: Arc<RTCRtpSender>,
    pli_tx: mpsc::Sender<()>,
    remb_tx: tokio::sync::watch::Sender<BitrateBps>,
    abr_tx: broadcast::Sender<AbrObservation>,
    mut shutdown_rx: oneshot::Receiver<()>,
    loopback_bypass: bool,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => {
                    debug!("rtcp reader: shutdown received");
                    return;
                }
                result = sender.read_rtcp() => {
                    match result {
                        Ok((packets, _attrs)) => handle_rtcp_batch(
                            &packets,
                            &pli_tx,
                            &remb_tx,
                            &abr_tx,
                            loopback_bypass,
                        ).await,
                        Err(e) => {
                            warn!(error = ?e, "rtcp reader: read_rtcp failed; stopping");
                            return;
                        }
                    }
                }
            }
        }
    });
}

/// Middle 32 bits of the current NTP timestamp (low 16 of seconds since
/// 1900 || high 16 of fractional second). Matches the `last_sender_report`
/// field's encoding so RTT can be derived without a 64-bit subtraction.
fn ntp_middle32_now() -> u32 {
    // NTP epoch (1900) → UNIX epoch (1970) offset = 70 y + 17 leap days.
    const NTP_UNIX_OFFSET_SECS: u64 = 2_208_988_800;
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs().wrapping_add(NTP_UNIX_OFFSET_SECS);
    let frac = (dur.subsec_nanos() as u64).saturating_mul(1u64 << 32) / 1_000_000_000;
    (((secs & 0xFFFF) << 16) as u32) | (((frac >> 16) & 0xFFFF) as u32)
}

/// Convert a `ReceptionReport` into the `Network` observation fields.
/// Loss is Q.8 → percentage; jitter is RTP timestamp units → ms via the
/// 90 kHz video clock; RTT uses the standard RFC 3550 derivation. RTT is
/// `None` until the receiver has acknowledged at least one of our SRs
/// (LSR=0 means "no SR seen yet").
fn report_to_network_obs(
    report: &rtcp::reception_report::ReceptionReport,
) -> AbrObservation {
    let loss_pct = (report.fraction_lost as f32) * 100.0 / 256.0;
    let jitter_ms = report.jitter / 90;
    let rtt_ms = if report.last_sender_report == 0 {
        None
    } else {
        let now32 = ntp_middle32_now();
        let elapsed = now32
            .wrapping_sub(report.last_sender_report)
            .wrapping_sub(report.delay);
        // 1/65536-second units → ms. >>16 gives whole seconds; multiply
        // first to keep sub-second precision.
        Some((((elapsed as u64) * 1000) >> 16) as u32)
    };
    AbrObservation::Network {
        remb_kbps: None,
        loss_pct: Some(loss_pct),
        rtt_ms,
        jitter_ms: Some(jitter_ms),
    }
}

async fn handle_rtcp_batch(
    packets: &[Box<dyn rtcp::packet::Packet + Send + Sync>],
    pli_tx: &mpsc::Sender<()>,
    remb_tx: &tokio::sync::watch::Sender<BitrateBps>,
    abr_tx: &broadcast::Sender<AbrObservation>,
    loopback_bypass: bool,
) {
    for pkt in packets {
        let any = pkt.as_any();
        if any
            .downcast_ref::<rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>()
            .is_some()
        {
            // Non-blocking: if the encoder task is slow to consume, one PLI is
            // as good as ten — we coalesce implicitly.
            let _ = pli_tx.try_send(());
            continue;
        }
        if let Some(remb) = any
            .downcast_ref::<rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate>(
            )
        {
            // Always emit the raw REMB observation so the stats SSE shows
            // what Chrome's BWE thinks, even when we're not honoring it.
            let _ = abr_tx.send(AbrObservation::Network {
                remb_kbps: Some((remb.bitrate as u32) / 1_000),
                loss_pct: None,
                rtt_ms: None,
                jitter_ms: None,
            });
            // Loopback bypass: skip the encoder-bitrate update entirely.
            // Chrome's GCC collapses to near-zero on same-machine paths;
            // honoring it would starve the encoder. Operator's tier
            // bitrate stays in effect via the initial `bitrate_tx` value.
            if loopback_bypass {
                continue;
            }
            // REMB's bitrate is f32 bps. Clamp to a safe range:
            // - Floor 6 Mbps (= MED tier preset): below this, screen
            //   content becomes unreadable regardless of any other
            //   encoder knob. Raised from 1.5 Mbps on 2026-05-13 after
            //   observing Chrome's GCC reporting REMB=3.9M on Cloudflared
            //   tunnel traffic despite 0% packet loss and a sustainable
            //   24 Mbps encoder target — Chrome's REMB is known to
            //   underestimate over tunneled paths. The 1.5 Mbps floor
            //   was originally chosen to "keep Low tier intact" but Low
            //   tier is itself 2.5 Mbps; the gap was load-bearing for
            //   nothing.
            // - Ceiling 30 Mbps: matches the HiDPI High tier
            //   (12 Mbps × 2 = 24 Mbps clamped at the tier_bitrate cap).
            let bps = (remb.bitrate as u32).clamp(6_000_000, 30_000_000);
            let _ = remb_tx.send(BitrateBps(bps));
            continue;
        }
        if let Some(rr) = any
            .downcast_ref::<rtcp::receiver_report::ReceiverReport>()
        {
            for report in &rr.reports {
                let _ = abr_tx.send(report_to_network_obs(report));
            }
            continue;
        }
        if let Some(sr) = any
            .downcast_ref::<rtcp::sender_report::SenderReport>()
        {
            // SRs from the receiver carry their own reception reports about
            // any media they're sending us. We don't currently send media
            // back, but the path is symmetric — propagate any reports we do
            // see so the controller sees them uniformly.
            for report in &sr.reports {
                let _ = abr_tx.send(report_to_network_obs(report));
            }
        }
    }
}

// ─── HEVC helpers ────────────────────────────────────────────────────────────
//
// All HEVC additions are gated on the `hevc` cargo feature. The `h264`-only
// build compiles with none of these symbols present.

/// MIME type string for HEVC in webrtc-rs 0.11 (the crate does not export one).
#[cfg(feature = "hevc")]
pub const MIME_TYPE_H265: &str = "video/H265";

/// Build the `RTCRtpCodecCapability` for HEVC Main profile, Level 5.1, 90 kHz
/// clock, single RTP stream (SRST mode), payload type 98.
///
/// `profile-id=1` = Main profile (ISO 14496-15 Table B.3).
/// `level-id=153` = Level 5.1 (30 × 5.1 = 153) — safe ceiling for HiDPI
/// captures; VideoToolbox AutoLevel picks the minimum actually needed.
/// `tier-flag=0` = Main tier.
/// `tx-mode=SRST` = Single RTP Stream on a Single Media Transport (unicast).
/// `profile-space=0` = standard HEVC profile space.
#[cfg(feature = "hevc")]
pub fn hevc_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H265.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line:
            "profile-id=1;level-id=153;tier-flag=0;tx-mode=SRST;profile-space=0".to_string(),
        rtcp_feedback: vec![],
    }
}

/// Create an HEVC track. Returns an `Arc<HevcTrack>` which holds an inner
/// `Arc<TrackLocalStaticRTP>` shared with the peer connection (via
/// `HevcTrack::as_track_local()`). The pipeline writer calls
/// `HevcTrack::write_sample` which runs `HevcPayloader` (RFC 7798) and
/// delegates to `TrackLocalStaticRTP::write_rtp_with_extensions`.
///
/// `TrackLocalStaticSample` is NOT used because webrtc-rs 0.11's `bind`
/// calls `payloader_for_codec()` which returns `ErrNoPayloaderForCodec` for
/// `video/H265` — only H.264/VP8/VP9/Opus have built-in payloaders.
#[cfg(feature = "hevc")]
pub fn new_hevc_track() -> Arc<crate::hevc_pipeline::HevcTrack> {
    Arc::new(crate::hevc_pipeline::HevcTrack::new(hevc_codec_capability()))
}

/// Build the raw `hvcC` payload bytes (HEVCDecoderConfigurationRecord, ISO
/// 14496-15 §8.3.3.1) from the parameter sets extracted on the first IDR.
///
/// The caller base64-encodes the result and sends it in `SignalOut::Pipeline`
/// as `hvcc_description_b64` so the SPA's `VideoDecoder.configure({
/// description })` can decode HEVC without waiting for in-band parameters.
///
/// Fixed-constant strategy: Main profile has well-known values for all
/// header fields. Parsing them from VPS/SPS bytes would be more correct
/// but would require a bitstream parser; the fixed values are always valid
/// for Main profile, Main tier, 4:2:0 chroma, 8-bit depth.
///
/// **Byte layout** (byte-exact per ISO 14496-15 §8.3.3.1; 22-byte fixed header
/// then numOfArrays=3 followed by VPS/SPS/PPS arrays):
/// ```text
/// [0x01]          configurationVersion = 1
/// [0x21]          profile_space=0(2)|tier=0(1)|profile_idc=1(5)
/// [0x60,0x00,0x00,0x00]  general_profile_compatibility_flags (Main bits 1+2)
/// [0x00;6]        constraint_indicator_flags (48 bits, no constraints)
/// [0x99]          general_level_idc = 153 (Level 5.1)
/// [0xF0,0x00]     min_spatial_segmentation_idc (12 bits=0, reserved=0xF)
/// [0xFC]          parallelismType (reserved 6 bits | type=0)
/// [0xFD]          chroma_format_idc=1 (4:2:0, reserved 6 bits)
/// [0xF8]          bit_depth_luma_minus8=0 (reserved 5 bits)
/// [0xF8]          bit_depth_chroma_minus8=0 (reserved 5 bits)
/// [0x00,0x00]     avgFrameRate = 0 (unspecified)
/// [0x0B]          constantFR(2)=0|numTemporal(3)=1|tempIdNested(1)=0|lenSzMinus1(2)=3
/// [0x03]          numOfArrays = 3  ← byte index 22
/// VPS array: [0xA0, 0x00, 0x01, size_hi, size_lo, vps_bytes...]
/// SPS array: [0xA1, 0x00, 0x01, size_hi, size_lo, sps_bytes...]
/// PPS array: [0xA2, 0x00, 0x01, size_hi, size_lo, pps_bytes...]
/// ```
///
/// Note: `0x0B` (not `0x03` as written in an earlier spec draft). Hand
/// calculation: `0b00_001_0_11` = 0×64 + 0×32 + 0×16 + 0×8 + 1×4 + 0×2 + 1×1
/// Wait — bits: [7:6]=constantFR=0b00, [5:3]=numTemporalLayers=0b001,
/// [2]=temporalIdNested=0b0, [1:0]=lengthSizeMinusOne=0b11
/// → 0b00_001_0_11 = 0x0B.
#[cfg(feature = "hevc")]
pub fn build_hvcc(params: &desktop::encoders::HevcParameterSets) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        32 + params.vps.len() + params.sps.len() + params.pps.len(),
    );
    // configurationVersion
    buf.push(0x01u8);
    // profile_space(2)=0 | tier(1)=0 | profile_idc(5)=1 → 0b00_0_00001 = 0x01.
    // Tier MUST stay 0 (Main tier) to match SDP fmtp `tier-flag=0`; a tier
    // mismatch between hvcC and SDP causes conformant decoders to reject.
    buf.push(0x01u8);
    // general_profile_compatibility_flags: Main profile sets bits 1 and 2
    // (big-endian u32: 0x60000000).
    buf.extend_from_slice(&0x6000_0000u32.to_be_bytes());
    // constraint_indicator_flags: 48 bits, all zero (no constraints for Main).
    buf.extend_from_slice(&[0u8; 6]);
    // general_level_idc = 153 (Level 5.1).
    buf.push(153u8);
    // min_spatial_segmentation_idc: 4 reserved bits (0xF) + 12-bit value=0
    // packed as u16 big-endian: 0xF000.
    buf.extend_from_slice(&0xF000u16.to_be_bytes());
    // parallelismType: 6 reserved bits (all 1) + 2-bit type=0 → 0xFC.
    buf.push(0xFCu8);
    // chroma_format_idc: 6 reserved bits (all 1) + 2-bit idc=1 (4:2:0) → 0xFD.
    buf.push(0xFDu8);
    // bit_depth_luma_minus8: 5 reserved bits (all 1) + 3-bit value=0 → 0xF8.
    buf.push(0xF8u8);
    // bit_depth_chroma_minus8: same → 0xF8.
    buf.push(0xF8u8);
    // avgFrameRate: u16 = 0 (unspecified).
    buf.extend_from_slice(&0u16.to_be_bytes());
    // Packed byte: constantFrameRate(2)=0 | numTemporalLayers(3)=1 |
    //              temporalIdNested(1)=0 | lengthSizeMinusOne(2)=3
    // → 0b00_001_0_11 = 0x0B.
    buf.push(0x0Bu8);
    // numOfArrays = 3 (VPS, SPS, PPS).
    buf.push(3u8);
    // NAL type constants: VPS=32 (0x20), SPS=33 (0x21), PPS=34 (0x22).
    // array_completeness=1 → high bit set; reserved=0; NAL_type in low 6 bits.
    for (nal_type, nalu) in [
        (32u8, &params.vps),
        (33u8, &params.sps),
        (34u8, &params.pps),
    ] {
        // array_completeness(1)=1 | reserved(1)=0 | NAL_type(6) → 0x80|nal_type
        buf.push(0x80 | nal_type);
        // numNalus: u16 BE = 1
        buf.extend_from_slice(&1u16.to_be_bytes());
        // NALU size: u16 BE
        buf.extend_from_slice(&(nalu.len() as u16).to_be_bytes());
        buf.extend_from_slice(nalu);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_capability_has_high_5_0_fmtp() {
        // Post 2026-05-13 quality uplift: High profile (`profile_idc=100`),
        // Level 5.0 (`level_idc=0x32`). High enables 8×8 transform + CABAC
        // for sharp text on screen content. Browsers in 2026 universally
        // hardware-decode H.264 High 4:2:0 over WebRTC.
        let cap = h264_codec_capability();
        assert_eq!(cap.mime_type, "video/H264");
        assert_eq!(cap.clock_rate, 90_000);
        assert!(cap.sdp_fmtp_line.contains("profile-level-id=640032"));
        assert!(cap.sdp_fmtp_line.contains("packetization-mode=1"));
        // Regression guard: the prior Baseline fmtp must NOT reappear.
        assert!(!cap.sdp_fmtp_line.contains("42e032"));
    }

    #[test]
    fn new_h264_track_has_matching_codec() {
        let t = new_h264_track();
        assert_eq!(t.codec().mime_type, "video/H264");
    }

    #[test]
    fn pli_force_allowed_cold_start() {
        // No prior IDR — first PLI must be honored regardless of any
        // notion of "now". Otherwise a flooded session never recovers.
        assert!(pli_force_allowed(None, Instant::now()));
    }

    #[test]
    fn pli_force_allowed_throttles_within_interval() {
        let t0 = Instant::now();
        // Half the interval — should reject.
        let t1 = t0 + PLI_MIN_INTERVAL / 2;
        assert!(!pli_force_allowed(Some(t0), t1));
        // Just shy of the interval — still rejected.
        let t2 = t0 + PLI_MIN_INTERVAL - Duration::from_millis(1);
        assert!(!pli_force_allowed(Some(t0), t2));
    }

    #[test]
    fn pli_force_allowed_releases_after_interval() {
        let t0 = Instant::now();
        // Exactly at the interval — boundary inclusive.
        let t1 = t0 + PLI_MIN_INTERVAL;
        assert!(pli_force_allowed(Some(t0), t1));
        // Well past — definitely allowed.
        let t2 = t0 + PLI_MIN_INTERVAL + Duration::from_secs(5);
        assert!(pli_force_allowed(Some(t0), t2));
    }

    // ── HEVC helpers ──────────────────────────────────────────────────────────

    #[cfg(feature = "hevc")]
    #[test]
    fn hevc_codec_capability_mime_and_fmtp() {
        let cap = hevc_codec_capability();
        assert_eq!(cap.mime_type, "video/H265");
        assert_eq!(cap.clock_rate, 90_000);
        // Must advertise Main profile and Level 5.1 so HiDPI captures are
        // not false-rejected by the remote SDP parser.
        assert!(
            cap.sdp_fmtp_line.contains("profile-id=1"),
            "fmtp must declare Main profile (profile-id=1)"
        );
        assert!(
            cap.sdp_fmtp_line.contains("level-id=153"),
            "fmtp must declare Level 5.1 (level-id=153)"
        );
        assert!(
            cap.sdp_fmtp_line.contains("tier-flag=0"),
            "fmtp must declare Main tier (tier-flag=0)"
        );
    }

    /// Verify the first 23 bytes of `build_hvcc` byte-for-byte against the
    /// ISO 14496-15 §8.3.3.1 layout. Regression guard — a single off-by-one
    /// here corrupts the `hvcC` box and causes `VideoDecoder.configure` to
    /// throw on the client side.
    ///
    /// Notably: byte 18 (index 17) is `0x0B`, NOT `0x03` as the phase-04
    /// spec draft mistakenly stated. Correct derivation:
    ///   constantFrameRate(2)=0b00 | numTemporalLayers(3)=0b001 |
    ///   temporalIdNested(1)=0b0  | lengthSizeMinusOne(2)=0b11
    ///   → 0b00_001_0_11 = 0x0B.
    #[cfg(feature = "hevc")]
    #[test]
    fn build_hvcc_header_bytes() {
        use desktop::encoders::HevcParameterSets;
        // Minimal synthetic parameter sets — contents don't affect the header.
        let params = HevcParameterSets {
            vps: bytes::Bytes::from_static(&[0x40, 0x01]),
            sps: bytes::Bytes::from_static(&[0x42, 0x01]),
            pps: bytes::Bytes::from_static(&[0x44, 0x01]),
            is_hardware: true,
        };
        let hvcc = build_hvcc(&params);

        // Expected first 22 bytes (fixed header, before numOfArrays).
        // Count: 1+1+4+6+1+2+1+1+1+1+2+1 = 22 bytes.
        #[rustfmt::skip]
        let expected_header: [u8; 22] = [
            0x01,                           // configurationVersion
            0x01,                           // profile_space=0|tier=0|profile_idc=1
            0x60, 0x00, 0x00, 0x00,         // general_profile_compatibility_flags (Main)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // constraint_indicator_flags (48 bits)
            0x99,                           // general_level_idc = 153
            0xF0, 0x00,                     // min_spatial_segmentation_idc (0xF000)
            0xFC,                           // parallelismType
            0xFD,                           // chroma_format_idc=1 (4:2:0)
            0xF8,                           // bit_depth_luma_minus8=0
            0xF8,                           // bit_depth_chroma_minus8=0
            0x00, 0x00,                     // avgFrameRate = 0
            0x0B,                           // constantFR|numTemporal|nested|lenSzMinus1
        ];
        assert!(
            hvcc.len() >= expected_header.len(),
            "hvcc too short: {} bytes",
            hvcc.len()
        );
        assert_eq!(
            &hvcc[..expected_header.len()],
            &expected_header,
            "hvcC header bytes mismatch — ISO 14496-15 layout violated"
        );

        // byte 22 (index 22) = numOfArrays = 3
        assert_eq!(hvcc[22], 3, "numOfArrays must be 3");

        // Verify each NAL array: completeness=1|reserved=0|nal_type, numNalus=1.
        let mut off = 23;
        for (expected_nal_type, nalu) in [
            (32u8, &params.vps),
            (33u8, &params.sps),
            (34u8, &params.pps),
        ] {
            assert_eq!(
                hvcc[off], 0x80 | expected_nal_type,
                "array header byte mismatch for NAL type {expected_nal_type}"
            );
            assert_eq!(
                u16::from_be_bytes([hvcc[off + 1], hvcc[off + 2]]),
                1,
                "numNalus must be 1"
            );
            let sz = u16::from_be_bytes([hvcc[off + 3], hvcc[off + 4]]) as usize;
            assert_eq!(sz, nalu.len(), "NALU size mismatch");
            assert_eq!(&hvcc[off + 5..off + 5 + sz], nalu.as_ref());
            off += 5 + sz;
        }
        assert_eq!(off, hvcc.len(), "trailing bytes in hvcC");
    }
}
