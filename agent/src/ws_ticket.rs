// Single-use WebSocket upgrade tickets (Phase 05 / H14).
//
// Browsers can't set `Authorization` on a WebSocket upgrade. The legacy
// workaround was carrying the long-lived api_key as a `Sec-WebSocket-Protocol`
// value (`["oxi-bearer-v1", <api_key>]`), which leaks the key to anything that
// can read DevTools / extension tabs / proxy logs.
//
// The ticket flow replaces it: SPA mints a 16-byte one-shot token via
// `POST /api/ws-ticket` (Bearer + CSRF gated), then opens the WS with
// `["oxi-ticket-v1", <ticket>]`. The agent claims the ticket atomically via
// `DashMap::remove`. Single-use is structural — the entry vanishes from the
// map on first claim, so any concurrent second attempt sees `None`. No
// separate AtomicBool needed.
//
// TTL is 60s — the SPA mints the ticket immediately before opening the WS.
// A periodic GC (30s cadence) sweeps stale entries so the map can't grow
// unbounded if a mint isn't followed by an upgrade.

use std::sync::Arc;

use dashmap::DashMap;
use rand::RngCore;

use crate::db::now_ts;

/// Subprotocol marker for ticket-based WS auth. SPA offers
/// `["oxi-ticket-v1", <hex>]`; the agent echoes the marker on accept so the
/// response header doesn't expose the ticket value.
pub const WS_TICKET_PROTOCOL: &str = "oxi-ticket-v1";

/// Time a freshly-minted ticket stays claimable, in seconds. Short on purpose
/// — the SPA mints just before `new WebSocket(...)`. 60s comfortably absorbs
/// network jitter without widening the replay window.
pub const TICKET_TTL_SECS: i64 = 60;

/// GC sweep cadence — runs as a background task started from `server_main`.
pub const TICKET_GC_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct TicketEntry {
    pub device_id: String,
    pub session_id: String,
    pub expires_at: i64,
}

pub type TicketStore = Arc<DashMap<String, TicketEntry>>;

pub fn new_store() -> TicketStore {
    Arc::new(DashMap::new())
}

/// Mint a fresh ticket bound to `device_id` + `session_id`. Returns the
/// ticket string (32-char lowercase hex) and its absolute expiry.
pub fn mint_ticket(store: &TicketStore, device_id: &str, session_id: &str) -> (String, i64) {
    let mut buf = [0u8; 16];
    rand::rng().fill_bytes(&mut buf);
    let ticket = hex::encode(buf);
    let expires_at = now_ts() + TICKET_TTL_SECS;
    store.insert(
        ticket.clone(),
        TicketEntry {
            device_id: device_id.to_string(),
            session_id: session_id.to_string(),
            expires_at,
        },
    );
    (ticket, expires_at)
}

/// Atomic single-use claim. Returns `Some(entry)` only if the ticket existed
/// AND had not expired. The entry is removed unconditionally on a hit — even
/// an expired ticket is consumed so a misbehaving client can't keep retrying.
pub fn claim_ticket(store: &TicketStore, ticket: &str) -> Option<TicketEntry> {
    let (_, entry) = store.remove(ticket)?;
    if entry.expires_at > now_ts() {
        Some(entry)
    } else {
        None
    }
}

/// Sweep expired entries. Cheap to run frequently — `DashMap::retain` shards
/// the iteration so other claims aren't blocked on the whole map.
pub fn gc_expired(store: &TicketStore) {
    let now = now_ts();
    store.retain(|_, entry| entry.expires_at > now);
}

/// Spawn the periodic GC task. Caller is responsible for keeping the returned
/// JoinHandle alive (typically the agent server task itself).
pub fn spawn_gc(store: TicketStore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
            TICKET_GC_INTERVAL_SECS,
        ));
        ticker.tick().await; // skip immediate fire
        loop {
            ticker.tick().await;
            gc_expired(&store);
        }
    })
}

/// Same parse contract as `auth::extract_bearer_from_ws_protocols` but for the
/// ticket marker. Browsers send the offered subprotocols as a comma-joined
/// header value; we accept either order and ignore extra whitespace.
pub fn extract_ticket_from_ws_protocols(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get("sec-websocket-protocol")?.to_str().ok()?;
    let mut saw_marker = false;
    let mut other: Option<String> = None;
    for part in raw.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p == WS_TICKET_PROTOCOL {
            saw_marker = true;
        } else if other.is_none() {
            other = Some(p.to_string());
        }
    }
    if saw_marker { other } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn mint_then_claim_returns_entry_and_consumes() {
        let s = new_store();
        let (t, _exp) = mint_ticket(&s, "dev-1", "sess-1");
        assert_eq!(t.len(), 32);
        let entry = claim_ticket(&s, &t).expect("first claim");
        assert_eq!(entry.device_id, "dev-1");
        assert_eq!(entry.session_id, "sess-1");
        // Second claim must miss — atomic single-use.
        assert!(claim_ticket(&s, &t).is_none(), "second claim leaked");
    }

    #[test]
    fn unknown_ticket_returns_none() {
        let s = new_store();
        assert!(claim_ticket(&s, "deadbeef".repeat(4).as_str()).is_none());
    }

    #[test]
    fn expired_ticket_rejected_and_consumed() {
        let s = new_store();
        let entry = TicketEntry {
            device_id: "d".into(),
            session_id: "s".into(),
            expires_at: now_ts() - 1,
        };
        s.insert("00112233445566778899aabbccddeeff".into(), entry);
        assert!(
            claim_ticket(&s, "00112233445566778899aabbccddeeff").is_none(),
            "expired must reject"
        );
        // Even though expired, the entry is removed so retries don't pile up.
        assert!(!s.contains_key("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn gc_sweeps_only_expired() {
        let s = new_store();
        let (live, _) = mint_ticket(&s, "d", "s");
        s.insert(
            "00112233445566778899aabbccddeeff".into(),
            TicketEntry {
                device_id: "d".into(),
                session_id: "s".into(),
                expires_at: now_ts() - 5,
            },
        );
        gc_expired(&s);
        assert!(s.contains_key(&live));
        assert!(!s.contains_key("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn extract_ticket_subprotocol_either_order() {
        let mut h = HeaderMap::new();
        h.insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("oxi-ticket-v1, abc123"),
        );
        assert_eq!(
            extract_ticket_from_ws_protocols(&h),
            Some("abc123".to_string())
        );
        h.insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("xyz789, oxi-ticket-v1"),
        );
        assert_eq!(
            extract_ticket_from_ws_protocols(&h),
            Some("xyz789".to_string())
        );
        h.insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("oxi-bearer-v1, abc123"),
        );
        assert!(extract_ticket_from_ws_protocols(&h).is_none());
    }

    #[test]
    fn concurrent_claims_only_one_wins() {
        // Hit the same ticket from many threads to exercise the atomic
        // remove. Exactly one Some is expected.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let s = new_store();
        let (t, _) = mint_ticket(&s, "d", "s");
        let hits = AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..16 {
                let s = s.clone();
                let t = t.clone();
                let hits = &hits;
                sc.spawn(move || {
                    if claim_ticket(&s, &t).is_some() {
                        hits.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
