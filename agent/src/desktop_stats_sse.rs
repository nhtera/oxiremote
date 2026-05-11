//! Phase-03 step 5: 1 Hz stats SSE for the active H.264 desktop session.
//!
//! Endpoint: `GET /api/hosts/{id}/desktop/stats` (tunnel-scoped, auth
//! required). Streams a `StatsSnapshot` JSON line every second for the
//! lifetime of the SSE connection. Stops when the browser disconnects,
//! when the desktop session is evicted, or when the SSE consumer can't
//! keep up (broadcast `Lagged`).
//!
//! The aggregator subscribes to the per-session `AbrObservation`
//! broadcast and folds the stream of partial observations into a
//! rolling snapshot. Each tick carries the latest known value of every
//! field; producers that haven't emitted yet show as `null` in JSON.
//!
//! This module produces visibility, not behavior — the controller
//! (step 3) consumes the same broadcast independently.
//!
//! No SPA bundle delta — overlay lands in step 6. Until then this is
//! useful by itself for dev tools / curl debugging.

#![cfg(feature = "h264")]

use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_stream::stream;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::time;
use tracing::debug;

use crate::desktop_abr::AbrObservation;

/// Tick interval for the aggregator. 1 Hz per phase-03 plan — fast
/// enough for a UX overlay, slow enough that even a 10-min session
/// stays under 600 SSE frames.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// One emit window's worth of stats. Fields that haven't been observed
/// at least once in the session's lifetime serialize as `null`. Schema
/// is intentionally flat so the SPA overlay can read it without any
/// nested-key bookkeeping.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StatsSnapshot {
    /// UNIX epoch ms at the moment the snapshot was assembled.
    pub ts_ms: u64,
    /// Active video pipeline label — currently always `"h264"` because
    /// only the h264 path emits observations. JPEG path lands in a
    /// later step; keep the field so the schema doesn't churn.
    pub pipeline: &'static str,
    /// REMB target the receiver is signaling (kbps, unclamped).
    pub remb_kbps: Option<u32>,
    /// Current encoder target bitrate (kbps) — the REMB-clamped value
    /// the encoder is actually configured to produce.
    pub bitrate_kbps: Option<u32>,
    /// Loss fraction observed in the most recent RR (0–100%).
    pub loss_pct: Option<f32>,
    /// RTT in ms (RFC 3550 derivation from RR).
    pub rtt_ms: Option<u32>,
    /// Inter-arrival jitter in ms (90 kHz video clock).
    pub jitter_ms: Option<u32>,
    /// p50 of `encode_ms` over the emit window.
    pub encode_ms_p50: Option<u32>,
    /// p95 of `encode_ms` over the emit window.
    pub encode_ms_p95: Option<u32>,
    /// Frames encoded in the emit window. Combined with `TICK_INTERVAL`
    /// this is the effective FPS the encoder is producing.
    pub frames_encoded: u32,
    /// Keyframes (IDRs) in the emit window. Spikes on PLI recovery /
    /// session-start; sustained values indicate bandwidth waste.
    pub keyframes: u32,
    /// Max BGRA queue depth observed during the window. Sustained
    /// non-zero indicates capture out-pacing the encoder.
    pub bgra_queue_depth_peak: u32,
    /// Max sample-writer queue depth observed during the window.
    /// Sustained non-zero indicates network saturation.
    pub sample_queue_depth_peak: u32,
}

/// Mutable state the aggregator carries across ticks. Snapshots are
/// derived from this on every emit; per-window counters reset after
/// emit, persistent network values (REMB/loss/RTT) carry over.
#[derive(Debug, Default)]
struct AggregatorState {
    // Persistent: latest known per-session network observations.
    last_remb_kbps: Option<u32>,
    last_loss_pct: Option<f32>,
    last_rtt_ms: Option<u32>,
    last_jitter_ms: Option<u32>,
    last_bitrate_kbps: Option<u32>,

    // Per-window (reset on emit).
    encode_ms_window: Vec<u32>,
    keyframes_window: u32,
    bgra_queue_peak_window: u32,
    sample_queue_peak_window: u32,
}

impl AggregatorState {
    fn fold(&mut self, obs: AbrObservation) {
        match obs {
            AbrObservation::Network {
                remb_kbps,
                loss_pct,
                rtt_ms,
                jitter_ms,
            } => {
                if let Some(v) = remb_kbps {
                    self.last_remb_kbps = Some(v);
                }
                if let Some(v) = loss_pct {
                    self.last_loss_pct = Some(v);
                }
                if let Some(v) = rtt_ms {
                    self.last_rtt_ms = Some(v);
                }
                if let Some(v) = jitter_ms {
                    self.last_jitter_ms = Some(v);
                }
            }
            AbrObservation::Encode {
                encode_ms,
                is_keyframe,
                bgra_queue_depth,
                sample_queue_depth,
                current_bitrate_kbps,
            } => {
                self.encode_ms_window.push(encode_ms);
                if is_keyframe {
                    self.keyframes_window = self.keyframes_window.saturating_add(1);
                }
                self.bgra_queue_peak_window = self
                    .bgra_queue_peak_window
                    .max(bgra_queue_depth as u32);
                self.sample_queue_peak_window = self
                    .sample_queue_peak_window
                    .max(sample_queue_depth as u32);
                self.last_bitrate_kbps = Some(current_bitrate_kbps);
            }
        }
    }

    fn emit(&mut self) -> StatsSnapshot {
        let frames_encoded = self.encode_ms_window.len() as u32;
        let (encode_ms_p50, encode_ms_p95) = percentiles(&mut self.encode_ms_window);

        let snapshot = StatsSnapshot {
            ts_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
                .unwrap_or(0),
            pipeline: "h264",
            remb_kbps: self.last_remb_kbps,
            bitrate_kbps: self.last_bitrate_kbps,
            loss_pct: self.last_loss_pct,
            rtt_ms: self.last_rtt_ms,
            jitter_ms: self.last_jitter_ms,
            encode_ms_p50,
            encode_ms_p95,
            frames_encoded,
            keyframes: self.keyframes_window,
            bgra_queue_depth_peak: self.bgra_queue_peak_window,
            sample_queue_depth_peak: self.sample_queue_peak_window,
        };

        // Reset per-window fields. Persistent network values carry over.
        self.encode_ms_window.clear();
        self.keyframes_window = 0;
        self.bgra_queue_peak_window = 0;
        self.sample_queue_peak_window = 0;

        snapshot
    }
}

/// Sort + return (p50, p95) — `None` for both when the window is empty.
/// Sorts in place to avoid a second allocation.
fn percentiles(samples: &mut [u32]) -> (Option<u32>, Option<u32>) {
    if samples.is_empty() {
        return (None, None);
    }
    samples.sort_unstable();
    let pick = |q: f32| {
        let idx = ((samples.len() as f32 - 1.0) * q).round() as usize;
        samples[idx]
    };
    (Some(pick(0.5)), Some(pick(0.95)))
}

/// Build the SSE response. Drives the aggregator on a 1 Hz `tokio::time::interval`,
/// drains pending observations between ticks via `try_recv`, and emits a
/// JSON-serialized snapshot per tick.
///
/// Termination conditions:
/// - Browser disconnects (the SSE stream is dropped → task exits).
/// - The producer side closes (session ended) → we surface the final
///   snapshot then end the stream so the SPA can show "session ended".
/// - The receiver lags (slow consumer) → log + continue. Snapshots may
///   have undercounted frames during the lag window; we don't try to
///   reconcile.
pub fn make_stream(
    mut rx: broadcast::Receiver<AbrObservation>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = stream! {
        let mut state = AggregatorState::default();
        let mut tick = time::interval(TICK_INTERVAL);
        // Skip the immediate-fire on the first tick — wait one full
        // window so the first emit isn't a near-empty snapshot.
        tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let snapshot = state.emit();
                    match serde_json::to_string(&snapshot) {
                        Ok(json) => yield Ok(Event::default().data(json)),
                        Err(e) => {
                            debug!(error = %e, "stats sse: snapshot serialize failed");
                        }
                    }
                }
                msg = rx.recv() => {
                    match msg {
                        Ok(obs) => state.fold(obs),
                        Err(broadcast::error::RecvError::Closed) => {
                            // Producer dropped — emit the final snapshot
                            // so the overlay shows the trailing state,
                            // then end the stream cleanly.
                            let final_snapshot = state.emit();
                            if let Ok(json) = serde_json::to_string(&final_snapshot) {
                                yield Ok(Event::default().data(json));
                            }
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!(skipped = n, "stats sse: receiver lagged");
                        }
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_empty() {
        let mut v: Vec<u32> = vec![];
        assert_eq!(percentiles(&mut v), (None, None));
    }

    #[test]
    fn percentiles_single() {
        let mut v = vec![42];
        assert_eq!(percentiles(&mut v), (Some(42), Some(42)));
    }

    #[test]
    fn percentiles_typical() {
        let mut v: Vec<u32> = (1..=100).collect();
        let (p50, p95) = percentiles(&mut v);
        // Round-half-to-even on 49.5 → 50; index 50 holds value 51.
        // Round on 94.05 → 94; index 94 holds value 95.
        assert_eq!(p50, Some(51));
        assert_eq!(p95, Some(95));
    }

    #[test]
    fn fold_network_then_emit_carries_persistent_state() {
        let mut s = AggregatorState::default();
        s.fold(AbrObservation::Network {
            remb_kbps: Some(5_000),
            loss_pct: None,
            rtt_ms: Some(45),
            jitter_ms: None,
        });
        let snap1 = s.emit();
        assert_eq!(snap1.remb_kbps, Some(5_000));
        assert_eq!(snap1.rtt_ms, Some(45));
        assert_eq!(snap1.frames_encoded, 0);

        // Next tick with no new network obs — REMB + RTT carry over,
        // window counters reset.
        let snap2 = s.emit();
        assert_eq!(snap2.remb_kbps, Some(5_000));
        assert_eq!(snap2.rtt_ms, Some(45));
        assert_eq!(snap2.frames_encoded, 0);
    }

    #[test]
    fn fold_encode_then_emit_resets_window_counters() {
        let mut s = AggregatorState::default();
        for ms in [10, 20, 30, 40, 50] {
            s.fold(AbrObservation::Encode {
                encode_ms: ms,
                is_keyframe: ms == 10,
                bgra_queue_depth: 2,
                sample_queue_depth: 1,
                current_bitrate_kbps: 6_000,
            });
        }
        let snap = s.emit();
        assert_eq!(snap.frames_encoded, 5);
        assert_eq!(snap.keyframes, 1);
        assert_eq!(snap.bitrate_kbps, Some(6_000));
        assert_eq!(snap.encode_ms_p50, Some(30));
        assert_eq!(snap.encode_ms_p95, Some(50));
        assert_eq!(snap.bgra_queue_depth_peak, 2);
        assert_eq!(snap.sample_queue_depth_peak, 1);

        // Next emit — window counters cleared, current_bitrate_kbps
        // persists (it's the last-known sticky value).
        let snap2 = s.emit();
        assert_eq!(snap2.frames_encoded, 0);
        assert_eq!(snap2.keyframes, 0);
        assert_eq!(snap2.bitrate_kbps, Some(6_000));
    }
}
