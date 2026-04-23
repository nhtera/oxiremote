use std::collections::VecDeque;

/// Bounded ring buffer of PTY output chunks with monotonic sequence numbering.
///
/// Each appended chunk gets the next sequence number. When total bytes exceed
/// `capacity_bytes`, oldest chunks are evicted. Consumers use `snapshot_since`
/// to request all data newer than a known seq; if the requested seq has been
/// evicted (or no seq is known), the full buffer is returned.
pub struct RingBuffer {
    capacity_bytes: usize,
    next_seq: u64,
    total_bytes: usize,
    chunks: VecDeque<Chunk>,
}

#[derive(Clone)]
pub struct Chunk {
    pub seq: u64,
    pub data: String,
}

/// Result of a snapshot query.
pub struct Snapshot {
    /// First seq included (inclusive).
    pub from_seq: u64,
    /// Last seq included (inclusive). Equals `from_seq - 1` when empty (no data).
    pub to_seq: u64,
    /// Concatenated data across the included chunks.
    pub data: String,
}

impl RingBuffer {
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes: capacity_bytes.max(4096),
            next_seq: 0,
            total_bytes: 0,
            chunks: VecDeque::new(),
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn byte_len(&self) -> usize {
        self.total_bytes
    }

    /// Append a chunk and return the assigned seq. Evicts oldest while over capacity.
    pub fn append(&mut self, data: String) -> u64 {
        let seq = self.next_seq + 1;
        self.next_seq = seq;
        self.total_bytes += data.len();
        self.chunks.push_back(Chunk { seq, data });
        while self.total_bytes > self.capacity_bytes && self.chunks.len() > 1 {
            if let Some(front) = self.chunks.pop_front() {
                self.total_bytes -= front.data.len();
            }
        }
        seq
    }

    /// Return all chunks with seq > `last_seq`. If `last_seq` is older than the
    /// oldest retained chunk (or None), return everything currently buffered.
    pub fn snapshot_since(&self, last_seq: Option<u64>) -> Snapshot {
        let oldest = self.chunks.front().map(|c| c.seq);
        let start_exclusive = match (last_seq, oldest) {
            (Some(ls), Some(first)) if ls + 1 >= first => ls,
            _ => 0,
        };

        let mut data = String::new();
        let mut from_seq: Option<u64> = None;
        let mut to_seq: u64 = start_exclusive;
        for c in &self.chunks {
            if c.seq > start_exclusive {
                if from_seq.is_none() {
                    from_seq = Some(c.seq);
                }
                data.push_str(&c.data);
                to_seq = c.seq;
            }
        }

        Snapshot {
            from_seq: from_seq.unwrap_or(start_exclusive + 1),
            to_seq,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_is_monotonic_and_starts_at_one() {
        let mut rb = RingBuffer::new(8 * 1024);
        assert_eq!(rb.append("a".into()), 1);
        assert_eq!(rb.append("b".into()), 2);
        assert_eq!(rb.append("c".into()), 3);
        assert_eq!(rb.last_seq(), 3);
    }

    #[test]
    fn overflow_evicts_oldest_but_preserves_seq() {
        let mut rb = RingBuffer::new(4096);
        for i in 0..10 {
            // 1 KB each → total 10 KB, capacity 4 KB → most are evicted
            rb.append("x".repeat(1024) + &format!("_{i}"));
        }
        assert!(rb.byte_len() <= 4096 + 1024, "byte_len bounded");
        assert_eq!(rb.last_seq(), 10);
        let front = rb.chunks.front().unwrap();
        assert!(front.seq > 1, "oldest chunk evicted");
    }

    #[test]
    fn snapshot_full_when_last_seq_none() {
        let mut rb = RingBuffer::new(8 * 1024);
        rb.append("hello ".into());
        rb.append("world".into());
        let snap = rb.snapshot_since(None);
        assert_eq!(snap.from_seq, 1);
        assert_eq!(snap.to_seq, 2);
        assert_eq!(snap.data, "hello world");
    }

    #[test]
    fn snapshot_incremental_returns_only_newer() {
        let mut rb = RingBuffer::new(8 * 1024);
        rb.append("a".into());
        rb.append("b".into());
        rb.append("c".into());
        let snap = rb.snapshot_since(Some(2));
        assert_eq!(snap.from_seq, 3);
        assert_eq!(snap.to_seq, 3);
        assert_eq!(snap.data, "c");
    }

    #[test]
    fn snapshot_full_when_last_seq_older_than_oldest() {
        let mut rb = RingBuffer::new(4096);
        for _ in 0..10 {
            rb.append("x".repeat(1024));
        }
        // Oldest retained seq has moved past 1; client's stale seq falls back to full replay.
        let oldest = rb.chunks.front().unwrap().seq;
        assert!(oldest > 1, "precondition: eviction actually happened");
        let snap = rb.snapshot_since(Some(1));
        assert_eq!(snap.from_seq, oldest);
        assert_eq!(snap.to_seq, rb.last_seq());
    }

    #[test]
    fn snapshot_empty_when_client_up_to_date() {
        let mut rb = RingBuffer::new(8 * 1024);
        rb.append("a".into());
        rb.append("b".into());
        let snap = rb.snapshot_since(Some(2));
        assert_eq!(snap.data, "");
        assert_eq!(snap.to_seq, 2);
    }
}
