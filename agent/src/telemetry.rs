// Lightweight operator telemetry — zero-overhead atomics + a fixed-size
// probe-latency ring. No heap allocation in the hot path; ring is a 16-slot
// array with a wrapping write index.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Number of probe latency samples kept (ring buffer capacity).
pub const LATENCY_RING_CAP: usize = 16;

/// Operator telemetry counters. Lives inside `AppState` behind an `Arc`.
pub struct TelemetryState {
    /// Cumulative bytes sent/received through the Cloudflare tunnel (2xx
    /// responses only). Counts response body bytes written to the TCP socket
    /// after axum serialises the response. Relaxed ordering — counter accuracy
    /// within a request batch is sufficient for operator dashboards.
    pub bytes_proxied: AtomicU64,

    /// How many times the tunnel has reconnected since agent boot. Incremented
    /// by the tunnel-spawn task each time `TunnelUrlChanged` fires after the
    /// first connection. First boot does not count as a reconnect.
    pub reconnect_count: AtomicU64,

    /// Monotonic `Instant` at which the tunnel last became healthy
    /// (`TunnelStep::Ready`). `None` until the first ready event.
    /// Wrapped in a `Mutex<Option<Instant>>` — written rarely (once per
    /// tunnel session), read on operator dashboard requests.
    pub tunnel_ready_at: Mutex<Option<Instant>>,

    /// Unix-epoch milliseconds of the most recent reconnect, or 0 when none.
    /// Stored as AtomicU64 so the dashboard endpoint can read it lock-free.
    pub last_reconnect_at_ms: AtomicU64,

    /// Rolling ring of HTTP health-probe latencies (ms). Wraps at
    /// `LATENCY_RING_CAP`. Written by `health_check` via `record_probe`.
    latencies: Mutex<[u64; LATENCY_RING_CAP]>,

    /// Next write index into the latency ring (wraps mod LATENCY_RING_CAP).
    latency_idx: AtomicU32,

    /// How many latency samples have been recorded. Capped at LATENCY_RING_CAP
    /// for the `snapshot_latencies` return value.
    latency_count: AtomicU32,
}

impl TelemetryState {
    pub fn new() -> Self {
        Self {
            bytes_proxied: AtomicU64::new(0),
            reconnect_count: AtomicU64::new(0),
            tunnel_ready_at: Mutex::new(None),
            last_reconnect_at_ms: AtomicU64::new(0),
            latencies: Mutex::new([0u64; LATENCY_RING_CAP]),
            latency_idx: AtomicU32::new(0),
            latency_count: AtomicU32::new(0),
        }
    }

    /// Add `bytes` to the cumulative tunnel byte counter. Relaxed — no
    /// cross-thread ordering dependency beyond atomic visibility.
    #[inline]
    pub fn add_bytes(&self, bytes: u64) {
        self.bytes_proxied.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a health-probe latency sample (ms). Overwrites the oldest slot.
    pub fn record_probe_latency(&self, ms: u64) {
        let idx = self
            .latency_idx
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |i| {
                Some((i + 1) % LATENCY_RING_CAP as u32)
            })
            .unwrap_or(0) as usize;

        if let Ok(mut ring) = self.latencies.lock() {
            ring[idx] = ms;
        }
        // Saturate at LATENCY_RING_CAP so snapshot_latencies knows how many
        // slots are populated.
        let _ = self.latency_count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
            Some(c.min(LATENCY_RING_CAP as u32 - 1) + 1)
        });
    }

    /// Mark tunnel as ready now — called when `TunnelStep::Ready` fires.
    /// After the first call, subsequent calls increment `reconnect_count` and
    /// record `last_reconnect_at_ms`.
    pub fn mark_tunnel_ready(&self) {
        let now = Instant::now();
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        let mut guard = self.tunnel_ready_at.lock().unwrap_or_else(|e| e.into_inner());
        let is_reconnect = guard.is_some();
        *guard = Some(now);
        drop(guard);

        if is_reconnect {
            self.reconnect_count.fetch_add(1, Ordering::Relaxed);
            self.last_reconnect_at_ms.store(now_ms, Ordering::Relaxed);
        }
    }

    /// Return a copy of all recorded latency samples (up to LATENCY_RING_CAP),
    /// in insertion order (oldest first within the ring).
    pub fn snapshot_latencies(&self) -> Vec<u64> {
        let count = self.latency_count.load(Ordering::Relaxed) as usize;
        if count == 0 {
            return vec![];
        }
        let ring = match self.latencies.lock() {
            Ok(g) => *g,
            Err(e) => *e.into_inner(),
        };
        // Read index points to the slot written NEXT, so the oldest entry is at
        // read_idx (if count == LATENCY_RING_CAP) or slot 0 (count < cap).
        let read_idx = self.latency_idx.load(Ordering::Relaxed) as usize;
        let cap = LATENCY_RING_CAP;
        let mut out = Vec::with_capacity(count);
        if count < cap {
            // Ring not yet full — slots 0..count are in order.
            out.extend_from_slice(&ring[..count]);
        } else {
            // Ring is full — read_idx is the next-write slot = oldest slot.
            for i in 0..cap {
                out.push(ring[(read_idx + i) % cap]);
            }
        }
        out
    }

    /// Uptime in seconds since the tunnel last became `Ready`. Returns 0 when
    /// the tunnel has never been ready.
    pub fn tunnel_uptime_s(&self) -> u64 {
        self.tunnel_ready_at
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed().as_secs()))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_counter_increments() {
        let t = TelemetryState::new();
        assert_eq!(t.bytes_proxied.load(Ordering::Relaxed), 0);
        t.add_bytes(1024);
        t.add_bytes(512);
        assert_eq!(t.bytes_proxied.load(Ordering::Relaxed), 1536);
    }

    #[test]
    fn latency_ring_wraps_correctly() {
        let t = TelemetryState::new();
        // Fill beyond capacity.
        for i in 0..20u64 {
            t.record_probe_latency(i * 10);
        }
        let snaps = t.snapshot_latencies();
        // Should have exactly LATENCY_RING_CAP samples.
        assert_eq!(snaps.len(), LATENCY_RING_CAP);
        // Last 16 values written are 40, 50, …, 190 ms.
        assert_eq!(snaps[0], 40);
        assert_eq!(snaps[LATENCY_RING_CAP - 1], 190);
    }

    #[test]
    fn latency_ring_partial_fill() {
        let t = TelemetryState::new();
        t.record_probe_latency(100);
        t.record_probe_latency(200);
        let snaps = t.snapshot_latencies();
        assert_eq!(snaps, vec![100, 200]);
    }

    #[test]
    fn reconnect_count_increments_on_second_ready() {
        let t = TelemetryState::new();
        // First ready — not a reconnect.
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 0);
        // Second ready — IS a reconnect.
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 1);
        t.mark_tunnel_ready();
        assert_eq!(t.reconnect_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn uptime_positive_after_mark_ready() {
        let t = TelemetryState::new();
        assert_eq!(t.tunnel_uptime_s(), 0);
        t.mark_tunnel_ready();
        // Uptime should be 0 (just set) — just assert it doesn't panic.
        let _ = t.tunnel_uptime_s();
    }
}
