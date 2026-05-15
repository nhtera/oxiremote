//! VP9 capture → encode → RTP pipeline (libvpx software, screen-content tuned).
//!
//! Mirrors `video_pipeline.rs` but uses `Vp9Encoder` + webrtc-rs's built-in
//! VP9 payloader (RFC 8741) via `TrackLocalStaticSample`. No custom payloader,
//! no `avcC` config blob (VP9 is self-describing — the keyframe carries the
//! sequence header inline).
//!
//! Phase-03 ships the streaming path. Active-map (16×16 dirty-rect skip — the
//! #1 CRD speed gap per `researcher-260514-2337-crd-vp9-av1-encoder-config.md`)
//! lands in phase-03b once the `screencapturekit` 1.5 dirty-rect API is
//! confirmed; until then VP9 encodes full frames every tick.

#![cfg(feature = "vp9")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use desktop::encoders::{BitrateBps, EncodedFrame, Vp9Encoder, Vp9SequenceInfo};
use desktop::{QualityTier, RawBgraFrame};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};
use webrtc::api::media_engine::MIME_TYPE_VP9;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

/// Default duration for the first sample (no prior frame to measure from).
/// 33 ms ≈ 30 fps. Subsequent samples carry the true elapsed wall-clock.
const FIRST_FRAME_DURATION_MS: u64 = 33;

/// Minimum spacing between PLI-driven IDRs. Same 1 s libwebrtc default as
/// the H.264 pipeline — caps the death-spiral on congested paths where the
/// decoder PLIs on every dropped frame.
const PLI_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Pure-function PLI throttle (matches `video_pipeline::pli_force_allowed`).
fn pli_force_allowed(last_idr_at: Option<Instant>, now: Instant) -> bool {
    match last_idr_at {
        None => true,
        Some(t) => now.duration_since(t) >= PLI_MIN_INTERVAL,
    }
}

pub type BgraFrame = RawBgraFrame;

/// SDP `fmtp` line for VP9. `profile-id=0` = 8-bit 4:2:0 (I420). Profile 1
/// (4:4:4 for sharper text rendering) is deferred to a future phase — would
/// need a separate `TrackLocalStaticSample` with its own codec capability.
const VP9_SDP_FMTP_LINE: &str = "profile-id=0";

/// Build the `RTCRtpCodecCapability` exposed in the SDP offer. webrtc-rs
/// `register_default_codecs` already claims PT 98 for VP9, so a session that
/// reuses the default MediaEngine doesn't need to call `register_codec`
/// manually — but exposing the helper keeps the API symmetric with
/// `h264_codec_capability`.
pub fn vp9_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_VP9.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: VP9_SDP_FMTP_LINE.to_string(),
        rtcp_feedback: vec![],
    }
}

/// Create a VP9 `TrackLocalStaticSample`. The returned Arc is shared between
/// the pipeline writer task and the peer connection.
pub fn new_vp9_track() -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        vp9_codec_capability(),
        "video".to_string(),
        "oxiremote-desktop-vp9".to_string(),
    ))
}

// ─── VP9 pipeline config ─────────────────────────────────────────────────────

/// Config for `spawn_vp9_pipeline`. Mirrors `VideoPipelineConfig` but drops
/// the `max_qp` + `params_tx` knobs (libvpx CBR doesn't expose per-frame QP
/// caps; VP9 has no out-of-band parameter set blob to surface).
pub struct Vp9PipelineConfig {
    pub width: u32,
    pub height: u32,
    pub initial_bitrate: BitrateBps,
    pub track: Arc<TrackLocalStaticSample>,
    pub bgra_rx: mpsc::Receiver<BgraFrame>,
    pub bitrate_rx: watch::Receiver<BitrateBps>,
    pub fps_rx: watch::Receiver<QualityTier>,
    pub shutdown_rx: oneshot::Receiver<()>,
    pub pli_rx: mpsc::Receiver<()>,
    /// One-shot: fires when the first keyframe encodes. Lets the session
    /// layer emit `SignalOut::Pipeline { mode: "vp9", hardware_accel: false }`
    /// once the encoder is verified to be producing output.
    pub seq_info_tx: oneshot::Sender<Vp9SequenceInfo>,
    /// Observability counters surfaced to the session-start IDR watchdog so
    /// fallback warnings can name which side broke (capture vs encoder).
    pub frames_encoded_ok: Option<Arc<AtomicU64>>,
    pub frames_encoded_err: Option<Arc<AtomicU64>>,
}

// ─── Spawn ───────────────────────────────────────────────────────────────────

/// Build the libvpx VP9 backend.
fn build_encoder(
    width: u32,
    height: u32,
    bitrate: BitrateBps,
) -> Result<Box<dyn Vp9Encoder>> {
    let enc = desktop::encoders::vp9_encoder::VpxVp9Encoder::new(width, height, bitrate)?;
    info!("vp9_pipeline: using libvpx (software, screen-content tuned)");
    Ok(Box::new(enc))
}

/// Spawn encoder thread + writer task. Returns immediately. Both tasks
/// terminate when `shutdown_rx` fires or when the channels close.
pub fn spawn_vp9_pipeline(mut cfg: Vp9PipelineConfig) {
    // Capacity 1 + writer drain-to-latest = drop-oldest semantics.
    let (sample_tx, sample_rx) = mpsc::channel::<EncodedFrame>(1);

    let track = cfg.track.clone();
    let writer_fps_rx = cfg.fps_rx.clone();
    tokio::spawn(writer_task(track, sample_rx, writer_fps_rx));

    std::thread::spawn(move || {
        let mut encoder_w = cfg.width;
        let mut encoder_h = cfg.height;
        let mut encoder = match build_encoder(encoder_w, encoder_h, cfg.initial_bitrate) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "vp9_pipeline: encoder build failed, pipeline stopping");
                return;
            }
        };
        let mut seq_info_tx = Some(cfg.seq_info_tx);
        let mut last_bitrate = cfg.initial_bitrate.0;
        let mut pli_pending = false;
        let mut last_idr_at: Option<Instant> = None;
        info!("vp9_pipeline: encoder task started");

        loop {
            if cfg.shutdown_rx.try_recv().is_ok() {
                info!("vp9_pipeline: shutdown received");
                break;
            }
            while cfg.pli_rx.try_recv().is_ok() {
                pli_pending = true;
            }
            if cfg.bitrate_rx.has_changed().unwrap_or(false) {
                let new_bps = cfg.bitrate_rx.borrow_and_update().0;
                if new_bps != last_bitrate {
                    if let Err(e) = encoder.set_bitrate(BitrateBps(new_bps)) {
                        warn!(error = %e, "vp9 encoder set_bitrate failed");
                    } else {
                        last_bitrate = new_bps;
                    }
                }
            }

            let Some(mut frame) = cfg.bgra_rx.blocking_recv() else {
                break;
            };
            // Drain-to-latest: discard older frames the capture loop produced
            // while we were encoding. force_idr OR-ed so a queued PLI isn't lost.
            let mut drained_force_idr = frame.force_idr;
            while let Ok(newer) = cfg.bgra_rx.try_recv() {
                drained_force_idr |= newer.force_idr;
                frame = newer;
            }

            // Rebuild encoder on dim change (xcap Windows physical-vs-logical
            // mismatch — same trip-wire as H.264 path).
            if frame.width != encoder_w || frame.height != encoder_h {
                info!(
                    old_width = encoder_w, old_height = encoder_h,
                    new_width = frame.width, new_height = frame.height,
                    "vp9_pipeline: capture dim changed, rebuilding encoder"
                );
                match build_encoder(frame.width, frame.height, BitrateBps(last_bitrate)) {
                    Ok(e) => {
                        encoder = e;
                        encoder_w = frame.width;
                        encoder_h = frame.height;
                    }
                    Err(e) => {
                        warn!(error = %e, "vp9 encoder rebuild on dim change failed");
                        if let Some(c) = &cfg.frames_encoded_err {
                            c.fetch_add(1, Ordering::Relaxed);
                        }
                        continue;
                    }
                }
            }

            let now_pli = Instant::now();
            let pli_honored = pli_pending && pli_force_allowed(last_idr_at, now_pli);
            let force_idr = drained_force_idr || pli_honored;

            // Phase-03b: push dirty-rect → active-map BEFORE encode. On a
            // forced keyframe we mark every block active so the IDR carries
            // the full picture, otherwise libvpx skips untouched 16×16
            // blocks (the #1 CRD speed gap on idle desktops).
            if let Err(e) = encoder.apply_dirty_rects(
                frame.width,
                frame.height,
                &frame.dirty_rects,
                force_idr,
            ) {
                warn!(error = %e, "vp9 encoder.apply_dirty_rects failed");
            }

            match encoder.encode(&frame.bytes, frame.width, frame.height, force_idr) {
                Ok(Some(encoded)) => {
                    if pli_honored {
                        pli_pending = false;
                    }
                    if encoded.is_keyframe {
                        last_idr_at = Some(now_pli);
                        if let Some(tx) = seq_info_tx.take() {
                            // Surface `is_hardware=false` once on first KF.
                            let _ = tx.send(encoder.sequence_info().unwrap_or(Vp9SequenceInfo { is_hardware: false }));
                        }
                    }
                    if let Some(c) = &cfg.frames_encoded_ok {
                        let n = c.fetch_add(1, Ordering::Relaxed) + 1;
                        if n == 1 {
                            info!(
                                width = frame.width,
                                height = frame.height,
                                is_keyframe = encoded.is_keyframe,
                                "vp9_pipeline: first frame encoded"
                            );
                        }
                    }
                    // Forward to writer task. try_send so a stalled writer
                    // doesn't block the encoder.
                    let _ = sample_tx.try_send(encoded);
                }
                Ok(None) => continue, // encoder dropped this frame
                Err(e) => {
                    warn!(error = %e, "vp9 encoder.encode failed");
                    if let Some(c) = &cfg.frames_encoded_err {
                        c.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        info!("vp9_pipeline: encoder task exiting");
    });
}

// ─── Writer task ─────────────────────────────────────────────────────────────

/// Async writer: drains `sample_rx`, builds `webrtc::media::Sample`, calls
/// `track.write_sample`. Paces send cadence at the current tier's frame
/// interval so RTP arrival at the browser is steady (avoids bursty SCK pacing
/// fattening Chrome's jitter buffer).
async fn writer_task(
    track: Arc<TrackLocalStaticSample>,
    mut sample_rx: mpsc::Receiver<EncodedFrame>,
    mut fps_rx: watch::Receiver<QualityTier>,
) {
    let mut last_send_at: Option<Instant> = None;
    while let Some(frame) = sample_rx.recv().await {
        // Drain-to-latest: discard older samples to keep the freshest in
        // hand. Combined with capacity-1 channel = drop-oldest semantics.
        let mut latest = frame;
        while let Ok(newer) = sample_rx.try_recv() {
            latest = newer;
        }

        let now = Instant::now();
        let duration_ms = match last_send_at {
            Some(prev) => (now - prev).as_millis().clamp(1, 1000) as u64,
            None => FIRST_FRAME_DURATION_MS,
        };
        last_send_at = Some(now);

        let sample = Sample {
            data: latest.annexb,
            duration: Duration::from_millis(duration_ms),
            ..Default::default()
        };
        if let Err(e) = track.write_sample(&sample).await {
            warn!(error = ?e, "vp9_pipeline: track.write_sample failed");
        }

        // Pace to the current quality tier's frame interval.
        let interval = desktop::frame_interval(*fps_rx.borrow_and_update());
        let elapsed = now.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }
    info!("vp9_pipeline: writer task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp9_capability_advertises_profile_0() {
        let cap = vp9_codec_capability();
        assert_eq!(cap.mime_type, MIME_TYPE_VP9);
        assert_eq!(cap.clock_rate, 90_000);
        assert!(cap.sdp_fmtp_line.contains("profile-id=0"));
    }

    #[test]
    fn pli_force_allowed_returns_true_when_no_prior_idr() {
        assert!(pli_force_allowed(None, Instant::now()));
    }

    #[test]
    fn pli_force_allowed_throttles_within_min_interval() {
        let now = Instant::now();
        let recent = now.checked_sub(Duration::from_millis(500)).expect("instant arithmetic");
        // 500 ms after IDR; PLI_MIN_INTERVAL is 1 s → should NOT allow another.
        assert!(!pli_force_allowed(Some(recent), now));
    }

    #[test]
    fn pli_force_allowed_permits_after_min_interval() {
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(2)).expect("instant arithmetic");
        assert!(pli_force_allowed(Some(old), now));
    }
}
