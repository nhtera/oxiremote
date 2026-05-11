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
use std::time::{Duration, SystemTime};

use anyhow::Result;
use desktop::encoders::{BitrateBps, EncodedFrame, H264Encoder, ParameterSets};
use desktop::{QualityTier, RawBgraFrame};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

/// Default duration for the very first sample (no prior frame to measure
/// from). 33 ms ≈ 30 fps — a reasonable seed at any tier; subsequent
/// samples carry the true elapsed wall-clock interval.
const FIRST_FRAME_DURATION_MS: u64 = 33;

/// Alias for the canonical capture-layer type. Kept as a re-export so the
/// transport-layer API (`VideoPipelineConfig`) stays self-contained.
pub type BgraFrame = RawBgraFrame;

/// Configuration for `spawn_video_pipeline`. All channels are owned by the
/// caller so they can tear down cleanly on session close.
pub struct VideoPipelineConfig {
    pub width: u32,
    pub height: u32,
    pub initial_bitrate: BitrateBps,
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
}

/// Build the `RTCRtpCodecCapability` we expose in the SDP offer — H.264
/// baseline profile, Level 5.0, 90 kHz clock, packetization-mode=1.
///
/// Level 5.0 (level_idc=0x32, max ~2560×1920 / 36 Mbps) covers HiDPI
/// captures on Retina displays. The encoder runs at `AutoLevel` so VT
/// emits the minimum level the actual resolution requires; advertising
/// 5.0 here lets the SDP negotiate that envelope without false-rejection
/// from browsers that strictly check `profile-level-id`. profile-iop=e0
/// keeps the constrained-baseline flags (sets 0/1/2) so old decoders
/// that only do baseline still accept the stream.
pub fn h264_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H264.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line:
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e032".to_string(),
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
fn build_encoder(
    width: u32,
    height: u32,
    bitrate: BitrateBps,
) -> Result<Box<dyn H264Encoder>> {
    #[cfg(target_os = "macos")]
    {
        match desktop::encoders::videotoolbox_encoder::VideoToolboxEncoder::new(
            width, height, bitrate,
        ) {
            Ok(enc) => {
                info!("video_pipeline: using VideoToolbox (hardware)");
                return Ok(Box::new(enc));
            }
            Err(e) => warn!(error = %e, "VideoToolbox init failed, falling back to OpenH264"),
        }
    }
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
        let mut encoder = match build_encoder(cfg.width, cfg.height, cfg.initial_bitrate) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "video_pipeline: encoder build failed, pipeline stopping");
                return;
            }
        };
        let mut params_tx = Some(cfg.params_tx);
        let mut last_bitrate = cfg.initial_bitrate.0;
        let mut pli_pending = false;
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

            let force_idr = drained_force_idr || pli_pending;
            let encoded = match encoder.encode(
                &frame.bytes,
                frame.width,
                frame.height,
                force_idr,
            ) {
                Ok(Some(f)) => {
                    if force_idr {
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

            // First-keyframe → forward SPS/PPS so the session layer can
            // build the avcC description blob for WebCodecs.
            if encoded.is_keyframe
                && let (Some(tx), Some(p)) = (params_tx.take(), encoder.parameter_sets())
            {
                let _ = tx.send(p);
            }

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
pub fn spawn_rtcp_reader(
    sender: Arc<RTCRtpSender>,
    pli_tx: mpsc::Sender<()>,
    remb_tx: tokio::sync::watch::Sender<BitrateBps>,
    mut shutdown_rx: oneshot::Receiver<()>,
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
                        Ok((packets, _attrs)) => handle_rtcp_batch(&packets, &pli_tx, &remb_tx).await,
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

async fn handle_rtcp_batch(
    packets: &[Box<dyn rtcp::packet::Packet + Send + Sync>],
    pli_tx: &mpsc::Sender<()>,
    remb_tx: &tokio::sync::watch::Sender<BitrateBps>,
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
            // REMB's bitrate is f32 bps. Clamp to a safe range:
            // - Floor 1.5 Mbps: a 500 Kbps floor on ~1 MP screen content
            //   reduces to 0.6 bpp and produces unreadable text. 1.5 Mbps
            //   keeps Low tier intact under transient congestion.
            // - Ceiling 15 Mbps: lets High tier (12 Mbps) breathe so REMB
            //   never artificially clips the configured target on LAN.
            let bps = (remb.bitrate as u32).clamp(1_500_000, 15_000_000);
            let _ = remb_tx.send(BitrateBps(bps));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_capability_has_baseline_3_1_fmtp() {
        let cap = h264_codec_capability();
        assert_eq!(cap.mime_type, "video/H264");
        assert_eq!(cap.clock_rate, 90_000);
        assert!(cap.sdp_fmtp_line.contains("profile-level-id=42e032"));
        assert!(cap.sdp_fmtp_line.contains("packetization-mode=1"));
    }

    #[test]
    fn new_h264_track_has_matching_codec() {
        let t = new_h264_track();
        assert_eq!(t.codec().mime_type, "video/H264");
    }
}
