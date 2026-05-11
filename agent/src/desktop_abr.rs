//! Adaptive-bitrate observation channel (phase-03 step 2).
//!
//! Producers in the video pipeline (RTCP reader, encoder task) emit
//! `AbrObservation` events as they see new data. Consumers (the controller
//! and the stats SSE) subscribe to the broadcast and assemble their own
//! state — observations carry only the fields the producer knows.
//!
//! The controller itself (zone state machine) and the SSE aggregator land
//! in step 3 + step 5 respectively; this module is just the wire format.
//!
//! No `cfg(feature = "h264")` gate here — observations are pipeline-agnostic.
//! The JPEG path can emit `Encode` events too (encode_ms = tile-build time)
//! once phase-03 extends to JPEG; until then only the H.264 path produces.

use tokio::sync::broadcast;

/// Capacity tuned for an encoder producing ≤60 events/s plus an RTCP reader
/// firing every ~1 s — 32 slots gives ≥0.5 s of headroom before slow
/// consumers see `Lagged`. Aligned with the existing `events::EventBus`
/// pattern: slow consumers skip rather than block producers.
pub const ABR_BROADCAST_CAPACITY: usize = 32;

/// A single observation from somewhere in the video pipeline. Variants are
/// `#[non_exhaustive]` so new fields don't break downstream consumers.
///
/// Producers (encoder, RTCP reader) populate these via `abr_tx.send`;
/// consumers (controller in step 3, stats SSE in step 5) read them. Until
/// those consumers exist, every field is "produced but never read", so
/// scope a single `#[allow(dead_code)]` here rather than tagging each
/// field individually.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(dead_code)]
pub enum AbrObservation {
    /// Network-side feedback from the receiver, emitted on each RTCP packet
    /// the RTP sender receives. At most one of REMB / loss / RTT may be
    /// `Some` per emit — REMB comes in standalone PSFB packets, loss + RTT
    /// come together in RR/SR reception reports.
    Network {
        /// Receiver-estimated max bitrate (kbps), already clamped to the
        /// pipeline's safe band by the caller.
        remb_kbps: Option<u32>,
        /// Fraction of packets lost since the prior RR (0.0–100.0). Derived
        /// from `ReceptionReport.fraction_lost` Q.8 (`u8 * 100 / 256`).
        loss_pct: Option<f32>,
        /// Round-trip time computed via the standard RFC 3550 formula:
        /// `RTT = (now_ntp_middle32 - last_sender_report - delay) / 65536`.
        /// `None` until at least one SR has been sent — we can't compute RTT
        /// before the first SR establishes the LSR timestamp.
        rtt_ms: Option<u32>,
        /// Inter-arrival jitter in RTP timestamp units, converted to ms via
        /// the 90 kHz video clock (`jitter / 90`).
        jitter_ms: Option<u32>,
    },

    /// Encoder-side observation, emitted once per encoded frame. The
    /// controller subsamples at 1 Hz; the stats SSE aggregates p50/p95
    /// over each emit window.
    Encode {
        /// Wall-clock time from `encode()` enter → encoded sample produced,
        /// in milliseconds. Includes VT callback latency.
        encode_ms: u32,
        /// True if this frame was an IDR (PLI or session-start force).
        is_keyframe: bool,
        /// Number of BGRA frames currently queued behind the encoder. The
        /// capture→encode mpsc has a bounded capacity; sustained non-zero
        /// values indicate the encoder can't keep up with capture.
        bgra_queue_depth: usize,
        /// Number of encoded frames queued for the writer task (drop-oldest
        /// bounded mpsc). Sustained non-zero values indicate the writer
        /// can't drain fast enough — typically a network saturation signal.
        sample_queue_depth: usize,
        /// Current encoder target bitrate (kbps). Lets consumers correlate
        /// observed loss/RTT with the bitrate that produced them.
        current_bitrate_kbps: u32,
    },
}

/// Construct the broadcast pair used to fan observations out to consumers
/// (controller + stats SSE). The receiver returned here is the producer's
/// own subscription handle — consumers call `tx.subscribe()` to attach.
pub fn channel() -> (broadcast::Sender<AbrObservation>, broadcast::Receiver<AbrObservation>) {
    broadcast::channel(ABR_BROADCAST_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_capacity_matches_constant() {
        let (tx, mut rx) = channel();
        // Send capacity + 1 events; the first one should be dropped on the
        // receiver side as `Lagged(1)`. This verifies the runtime capacity
        // matches the documented constant.
        for i in 0..(ABR_BROADCAST_CAPACITY as u32 + 1) {
            tx.send(AbrObservation::Network {
                remb_kbps: Some(i),
                loss_pct: None,
                rtt_ms: None,
                jitter_ms: None,
            })
            .unwrap();
        }
        let first = rx.try_recv();
        assert!(
            matches!(first, Err(broadcast::error::TryRecvError::Lagged(1))),
            "expected Lagged(1) on capacity overflow, got {first:?}"
        );
    }

    #[test]
    fn observations_are_clone() {
        // Compile-time guarantee — Clone is required so the broadcast
        // receiver can fan out to multiple consumers without taking
        // ownership.
        fn assert_clone<T: Clone>() {}
        assert_clone::<AbrObservation>();
    }
}
