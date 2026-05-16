//! AV1 capture → encode → RTP pipeline (libaom software, screen-content tuned).
//!
//! Mirrors `vp9_pipeline.rs`: `Av1Encoder` + webrtc-rs's built-in AV1
//! payloader (RFC 9606 OBU fragmentation, registered at PT 41 by
//! `register_default_codecs`) via `TrackLocalStaticSample`. No custom
//! payloader module needed — webrtc-rs 0.11's `rtp::codecs::av1::Av1Payloader`
//! is functional and maps via `payloader_for_codec(MIME_TYPE_AV1)`.

#![cfg(feature = "av1")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use desktop::encoders::{Av1Encoder, Av1SequenceInfo, BitrateBps, EncodedFrame};
use desktop::{QualityTier, RawBgraFrame};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{info, warn};
use webrtc::api::media_engine::MIME_TYPE_AV1;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::desktop_abr::AbrObservation;

const FIRST_FRAME_DURATION_MS: u64 = 33;
const PLI_MIN_INTERVAL: Duration = Duration::from_secs(1);

fn pli_force_allowed(last_idr_at: Option<Instant>, now: Instant) -> bool {
    match last_idr_at {
        None => true,
        Some(t) => now.duration_since(t) >= PLI_MIN_INTERVAL,
    }
}

pub type BgraFrame = RawBgraFrame;

/// SDP `fmtp` for AV1 Main profile, level-idx=8 (Level 4.0, 1080p60-capable),
/// tier=0 (Main). Mirrors what `register_default_codecs` advertises plus
/// explicit level/tier so the SPA can size its decoder appropriately.
const AV1_SDP_FMTP_LINE: &str = "profile=0;level-idx=8;tier=0";

pub fn av1_codec_capability() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_AV1.to_string(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: AV1_SDP_FMTP_LINE.to_string(),
        rtcp_feedback: vec![],
    }
}

pub fn new_av1_track() -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        av1_codec_capability(),
        "video".to_string(),
        "oxiremote-desktop-av1".to_string(),
    ))
}

pub struct Av1PipelineConfig {
    pub width: u32,
    pub height: u32,
    pub initial_bitrate: BitrateBps,
    pub track: Arc<TrackLocalStaticSample>,
    pub bgra_rx: mpsc::Receiver<BgraFrame>,
    pub bitrate_rx: watch::Receiver<BitrateBps>,
    pub fps_rx: watch::Receiver<QualityTier>,
    pub shutdown_rx: oneshot::Receiver<()>,
    pub pli_rx: mpsc::Receiver<()>,
    pub seq_info_tx: oneshot::Sender<Av1SequenceInfo>,
    pub frames_encoded_ok: Option<Arc<AtomicU64>>,
    pub frames_encoded_err: Option<Arc<AtomicU64>>,
    /// Per-frame ABR observation publisher; see `vp9_pipeline::Vp9PipelineConfig`
    /// for the rationale.
    pub abr_tx: broadcast::Sender<AbrObservation>,
}

fn build_encoder(
    width: u32,
    height: u32,
    bitrate: BitrateBps,
) -> Result<Box<dyn Av1Encoder>> {
    let enc = desktop::encoders::av1_encoder::AomAv1Encoder::new(width, height, bitrate)?;
    info!("av1_pipeline: using libaom (software, screen-content tuned)");
    Ok(Box::new(enc))
}

pub fn spawn_av1_pipeline(mut cfg: Av1PipelineConfig) {
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
                warn!(error = %e, "av1_pipeline: encoder build failed, pipeline stopping");
                return;
            }
        };
        let mut seq_info_tx = Some(cfg.seq_info_tx);
        let mut last_bitrate = cfg.initial_bitrate.0;
        let mut pli_pending = false;
        let mut last_idr_at: Option<Instant> = None;
        info!("av1_pipeline: encoder task started");

        loop {
            if cfg.shutdown_rx.try_recv().is_ok() {
                info!("av1_pipeline: shutdown received");
                break;
            }
            while cfg.pli_rx.try_recv().is_ok() {
                pli_pending = true;
            }
            if cfg.bitrate_rx.has_changed().unwrap_or(false) {
                let new_bps = cfg.bitrate_rx.borrow_and_update().0;
                if new_bps != last_bitrate {
                    if let Err(e) = encoder.set_bitrate(BitrateBps(new_bps)) {
                        warn!(error = %e, "av1 encoder set_bitrate failed");
                    } else {
                        last_bitrate = new_bps;
                    }
                }
            }

            let Some(mut frame) = cfg.bgra_rx.blocking_recv() else { break; };
            let mut drained_force_idr = frame.force_idr;
            while let Ok(newer) = cfg.bgra_rx.try_recv() {
                drained_force_idr |= newer.force_idr;
                frame = newer;
            }

            // Snapshot bgra queue depth AFTER drain — mirrors h264/vp9 path.
            let bgra_queue_depth = cfg.bgra_rx.len();

            if frame.width != encoder_w || frame.height != encoder_h {
                info!(
                    old_width = encoder_w, old_height = encoder_h,
                    new_width = frame.width, new_height = frame.height,
                    "av1_pipeline: capture dim changed, rebuilding encoder"
                );
                match build_encoder(frame.width, frame.height, BitrateBps(last_bitrate)) {
                    Ok(e) => {
                        encoder = e;
                        encoder_w = frame.width;
                        encoder_h = frame.height;
                    }
                    Err(e) => {
                        warn!(error = %e, "av1 encoder rebuild on dim change failed");
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
            // forced keyframe we mark every block active so the keyframe
            // carries the full picture, otherwise libaom skips untouched
            // 16×16 blocks via AOME_SET_ACTIVEMAP.
            if let Err(e) = encoder.apply_dirty_rects(
                frame.width,
                frame.height,
                &frame.dirty_rects,
                force_idr,
            ) {
                warn!(error = %e, "av1 encoder.apply_dirty_rects failed");
            }

            let encode_started = Instant::now();
            match encoder.encode(&frame.bytes, frame.width, frame.height, force_idr) {
                Ok(Some(encoded)) => {
                    if pli_honored { pli_pending = false; }
                    if encoded.is_keyframe {
                        last_idr_at = Some(now_pli);
                        if let Some(tx) = seq_info_tx.take() {
                            let _ = tx.send(
                                encoder.sequence_info()
                                    .unwrap_or(Av1SequenceInfo { is_hardware: false })
                            );
                        }
                    }
                    if let Some(c) = &cfg.frames_encoded_ok {
                        let n = c.fetch_add(1, Ordering::Relaxed) + 1;
                        if n == 1 {
                            info!(
                                width = frame.width,
                                height = frame.height,
                                is_keyframe = encoded.is_keyframe,
                                "av1_pipeline: first frame encoded"
                            );
                        }
                    }
                    let encode_ms =
                        encode_started.elapsed().as_millis().min(u32::MAX as u128) as u32;
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
                    let _ = sample_tx.try_send(encoded);
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(error = %e, "av1 encoder.encode failed");
                    if let Some(c) = &cfg.frames_encoded_err {
                        c.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        info!("av1_pipeline: encoder task exiting");
    });
}

async fn writer_task(
    track: Arc<TrackLocalStaticSample>,
    mut sample_rx: mpsc::Receiver<EncodedFrame>,
    mut fps_rx: watch::Receiver<QualityTier>,
) {
    let mut last_send_at: Option<Instant> = None;
    while let Some(frame) = sample_rx.recv().await {
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
            warn!(error = ?e, "av1_pipeline: track.write_sample failed");
        }
        let interval = desktop::frame_interval(*fps_rx.borrow_and_update());
        let elapsed = now.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }
    info!("av1_pipeline: writer task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av1_capability_advertises_main_profile() {
        let cap = av1_codec_capability();
        assert_eq!(cap.mime_type, MIME_TYPE_AV1);
        assert_eq!(cap.clock_rate, 90_000);
        assert!(cap.sdp_fmtp_line.contains("profile=0"));
        assert!(cap.sdp_fmtp_line.contains("level-idx=8"));
    }

    #[test]
    fn pli_throttle_matches_vp9() {
        assert!(pli_force_allowed(None, Instant::now()));
        let now = Instant::now();
        assert!(!pli_force_allowed(
            now.checked_sub(Duration::from_millis(500)),
            now
        ));
        assert!(pli_force_allowed(
            now.checked_sub(Duration::from_secs(2)),
            now
        ));
    }
}
