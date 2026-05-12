//! HEVC (H.265) capture → encode → RTP pipeline.
//!
//! Mirrors `video_pipeline.rs` but uses `HevcEncoder` + `HevcPayloader` +
//! `HevcParameterSets` instead of the H.264 equivalents. macOS-only encoder
//! backend (VideoToolbox); no software HEVC fallback — caller falls back to
//! H.264 on `Err`. All items gated on `#[cfg(feature = "hevc")]`.

#![cfg(feature = "hevc")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use desktop::encoders::{BitrateBps, EncodedFrame, HevcEncoder, HevcParameterSets};
use desktop::QualityTier;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{info, warn};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;

use crate::desktop_abr::AbrObservation;
use crate::hevc_payloader::HevcPayloader;
use crate::video_pipeline::BgraFrame;

// ─── HevcTrack ────────────────────────────────────────────────────────────────

/// RTP track for HEVC that pairs a `TrackLocalStaticRTP` (webrtc-rs binding
/// lifecycle) with a `HevcPayloader` (RFC 7798 FU/AP packetization).
///
/// `TrackLocalStaticSample` cannot be used for H.265 because webrtc-rs 0.11's
/// `bind` calls `payloader_for_codec()` which returns `ErrNoPayloaderForCodec`
/// for `video/H265` (only H.264/VP8/VP9/Opus are built-in). We wrap
/// `TrackLocalStaticRTP` and drive packetization ourselves in `write_sample`.
///
/// `TrackLocalStaticRTP::write_rtp_with_extensions` injects SSRC and PayloadType
/// per binding, so our RTP packets are built with ssrc=0/pt=0.
pub struct HevcTrack {
    /// Inner RTP track shared with the webrtc-rs peer connection.
    /// `Arc` so the session layer can coerce it to `Arc<dyn TrackLocal + Send + Sync>`
    /// without wrapping `HevcTrack` in another `Arc` layer.
    pub rtp: Arc<TrackLocalStaticRTP>,
    /// Monotonically increasing sequence number counter (wrapping u16).
    seq: AtomicU16,
    /// RTP timestamp base; advanced by 90 kHz ticks per `write_sample` call.
    timestamp: AtomicU32,
}

impl HevcTrack {
    pub fn new(codec: RTCRtpCodecCapability) -> Self {
        Self {
            rtp: Arc::new(TrackLocalStaticRTP::new(
                codec,
                "video".to_string(),
                "oxiremote-desktop-hevc".to_string(),
            )),
            seq: AtomicU16::new(rand::random()),
            timestamp: AtomicU32::new(seed_timestamp()),
        }
    }

    /// Coerce the inner RTP track to `Arc<dyn TrackLocal + Send + Sync>` for
    /// `RTCPeerConnection::add_transceiver_from_track`. The returned Arc shares
    /// ownership of the same `TrackLocalStaticRTP` used by `write_sample`.
    pub fn as_track_local(&self) -> Arc<dyn TrackLocal + Send + Sync> {
        self.rtp.clone()
    }

    /// Write one Annex-B HEVC frame as one or more RTP packets (RFC 7798).
    /// All packets in a single AU share the same timestamp; the marker bit is
    /// set on the last packet (RFC 3550 §5.1).
    pub async fn write_sample(&self, data: &Bytes, duration: Duration) -> Result<()> {
        use webrtc::rtp::packetizer::Payloader as _;

        let mut payloader = HevcPayloader;
        let rtp_payloads = payloader
            .payload(1200, data)
            .map_err(|e| anyhow::anyhow!("HEVC payloader: {:?}", e))?;

        if rtp_payloads.is_empty() {
            return Ok(());
        }

        // Advance timestamp by 90 kHz ticks for this sample's duration.
        let ts_delta = (duration.as_secs_f64() * 90_000.0) as u32;
        let ts = self.timestamp.fetch_add(ts_delta, Ordering::Relaxed);

        let n = rtp_payloads.len();
        for (i, payload) in rtp_payloads.into_iter().enumerate() {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let is_last = i + 1 == n;
            let pkt = webrtc::rtp::packet::Packet {
                header: webrtc::rtp::header::Header {
                    sequence_number: seq,
                    timestamp: ts,
                    marker: is_last, // RFC 3550 §5.1: mark last packet of AU
                    // SSRC and PT are overwritten per binding by
                    // TrackLocalStaticRTP::write_rtp_with_extensions.
                    ssrc: 0,
                    payload_type: 0,
                    ..Default::default()
                },
                payload,
            };
            if let Err(e) = self.rtp.write_rtp_with_extensions(&pkt, &[]).await {
                warn!(error = ?e, "hevc_track: write_rtp failed");
            }
        }
        Ok(())
    }
}

/// Seed the RTP timestamp from wall clock for cross-reconnect uniqueness.
fn seed_timestamp() -> u32 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as u32)
        .wrapping_mul(90_000)
        .wrapping_add(dur.subsec_micros())
}

// ─── HevcPipelineConfig ───────────────────────────────────────────────────────

/// Configuration for `spawn_hevc_pipeline`. Mirrors `VideoPipelineConfig`
/// but uses `HevcParameterSets` for the first-IDR oneshot.
pub struct HevcPipelineConfig {
    pub width: u32,
    pub height: u32,
    pub initial_bitrate: BitrateBps,
    pub initial_max_qp: Option<u32>,
    pub track: Arc<HevcTrack>,
    pub bgra_rx: mpsc::Receiver<BgraFrame>,
    pub bitrate_rx: watch::Receiver<BitrateBps>,
    pub fps_rx: watch::Receiver<QualityTier>,
    pub shutdown_rx: oneshot::Receiver<()>,
    pub pli_rx: mpsc::Receiver<()>,
    /// Receives VPS+SPS+PPS on the first IDR so the session layer can build
    /// the `hvcC` config blob and send it over the signaling WS.
    pub params_tx: oneshot::Sender<HevcParameterSets>,
    pub abr_tx: broadcast::Sender<AbrObservation>,
}

// ─── build_hevc_encoder ───────────────────────────────────────────────────────

/// Construct the HEVC encoder backend. macOS-only (VideoToolbox); returns
/// `Err` on non-macOS or VT init failure. No software HEVC fallback by design:
/// failing closed avoids CPU-melting software HEVC on hardware that lacks
/// the VideoToolbox codec.
pub fn build_hevc_encoder(
    width: u32,
    height: u32,
    bitrate: BitrateBps,
    max_qp: Option<u32>,
) -> Result<Box<dyn HevcEncoder>> {
    #[cfg(target_os = "macos")]
    {
        let enc = desktop::encoders::videotoolbox_hevc_encoder::VideoToolboxHevcEncoder::new(
            width, height, bitrate, max_qp,
        )?;
        info!(max_qp = ?max_qp, "hevc_pipeline: using VideoToolbox HEVC (hardware)");
        Ok(Box::new(enc))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (width, height, bitrate, max_qp);
        Err(anyhow::anyhow!(
            "hevc_pipeline: HEVC encoder not supported on this platform"
        ))
    }
}

// ─── spawn_hevc_pipeline ─────────────────────────────────────────────────────

/// PLI rate-limit: mirrors H.264's 1-second throttle. Prevents PLI storms
/// from flooding the bitrate budget with keyframes.
const PLI_MIN_INTERVAL: Duration = Duration::from_secs(1);

fn pli_force_allowed(last_idr_at: Option<Instant>, now: Instant) -> bool {
    match last_idr_at {
        None => true,
        Some(t) => now.duration_since(t) >= PLI_MIN_INTERVAL,
    }
}

/// Spawn the HEVC encode + write tasks. Returns immediately. Both tasks
/// terminate when `shutdown_rx` fires or when the channels close.
///
/// `spawn_rtcp_reader` from `video_pipeline.rs` is codec-agnostic and is
/// used as-is for PLI + REMB feedback (same 6 Mbps floor, 30 Mbps ceiling).
pub fn spawn_hevc_pipeline(mut cfg: HevcPipelineConfig) {
    // Channel cap=1 + drain-to-latest in writer → drop-oldest semantics.
    let (sample_tx, sample_rx) = mpsc::channel::<EncodedFrame>(1);

    // Writer task — async, owns the track Arc.
    let track = cfg.track.clone();
    let writer_fps_rx = cfg.fps_rx.clone();
    tokio::spawn(hevc_writer_task(track, sample_rx, writer_fps_rx));

    // Encoder task — blocking thread (VT encoder is synchronous).
    std::thread::spawn(move || {
        let mut encoder = match build_hevc_encoder(
            cfg.width,
            cfg.height,
            cfg.initial_bitrate,
            cfg.initial_max_qp,
        ) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "hevc_pipeline: encoder build failed, pipeline stopping");
                return;
            }
        };
        let mut params_tx = Some(cfg.params_tx);
        let mut last_bitrate = cfg.initial_bitrate.0;
        let mut pli_pending = false;
        let mut last_idr_at: Option<Instant> = None;
        info!("hevc_pipeline: encoder task started");

        loop {
            if cfg.shutdown_rx.try_recv().is_ok() {
                info!("hevc_pipeline: shutdown received");
                break;
            }

            while cfg.pli_rx.try_recv().is_ok() {
                pli_pending = true;
            }

            if cfg.bitrate_rx.has_changed().unwrap_or(false) {
                let new_bps = cfg.bitrate_rx.borrow_and_update().0;
                if new_bps != last_bitrate {
                    if let Err(e) = encoder.set_bitrate(BitrateBps(new_bps)) {
                        warn!(error = %e, "hevc encoder set_bitrate failed");
                    } else {
                        last_bitrate = new_bps;
                    }
                }
            }

            let Some(mut frame) = cfg.bgra_rx.blocking_recv() else {
                break; // capture loop shut down
            };
            // Drain stale frames; OR force_idr flags so no queued PLI is lost.
            let mut drained_force_idr = frame.force_idr;
            while let Ok(newer) = cfg.bgra_rx.try_recv() {
                drained_force_idr |= newer.force_idr;
                frame = newer;
            }
            let bgra_queue_depth = cfg.bgra_rx.len();

            let now_pli = Instant::now();
            let pli_honored = pli_pending && pli_force_allowed(last_idr_at, now_pli);
            let force_idr = drained_force_idr || pli_honored;

            let encode_started = Instant::now();
            let encoded = match encoder.encode(&frame.bytes, frame.width, frame.height, force_idr) {
                Ok(Some(f)) => {
                    if pli_honored {
                        pli_pending = false;
                    }
                    f
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(error = %e, "hevc encoder.encode failed");
                    continue;
                }
            };
            let encode_ms = encode_started.elapsed().as_millis().min(u32::MAX as u128) as u32;

            if encoded.is_keyframe {
                last_idr_at = Some(Instant::now());
            }

            // First IDR → forward parameter sets to the session layer.
            if encoded.is_keyframe
                && let (Some(tx), Some(ps)) = (params_tx.take(), encoder.parameter_sets())
            {
                let _ = tx.send(ps);
            }

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

            if sample_tx.try_send(encoded).is_err() {
                tracing::debug!("hevc_pipeline: writer backpressure, dropping frame");
            }
        }
        drop(sample_tx);
    });
}

/// Consume `EncodedFrame`s and write them to the HEVC track.
/// Implements the same paced-send logic as `video_pipeline::writer_task`.
async fn hevc_writer_task(
    track: Arc<HevcTrack>,
    mut sample_rx: mpsc::Receiver<EncodedFrame>,
    fps_rx: watch::Receiver<QualityTier>,
) {
    let mut last_send: Option<Instant> = None;
    while let Some(mut frame) = sample_rx.recv().await {
        // Drain stale samples (drop-oldest semantics).
        while let Ok(newer) = sample_rx.try_recv() {
            frame = newer;
        }

        // Paced send — hold until at least `target_interval` since last send.
        let target_interval = desktop::frame_interval(*fps_rx.borrow());
        if let Some(prev) = last_send {
            let elapsed = prev.elapsed();
            if elapsed < target_interval {
                tokio::time::sleep(target_interval - elapsed).await;
            }
        }
        // Re-drain after pacing sleep.
        while let Ok(newer) = sample_rx.try_recv() {
            frame = newer;
        }

        let now = Instant::now();
        let duration = match last_send {
            Some(prev) => now
                .duration_since(prev)
                .clamp(Duration::from_millis(1), Duration::from_millis(200)),
            None => Duration::from_millis(33),
        };
        last_send = Some(now);

        if let Err(e) = track.write_sample(&frame.annexb, duration).await {
            warn!(error = ?e, "hevc_track.write_sample failed");
        }
    }
    info!("hevc_pipeline: writer task exiting");
}
