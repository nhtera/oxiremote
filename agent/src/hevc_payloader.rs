// RFC 7798 — RTP Payload Format for HEVC
// Implements the webrtc-rs `Payloader` trait so `TrackLocalStaticSample`
// can fragment Annex-B HEVC bytestreams into valid RTP payloads.
//
// Three packet modes (per RFC 7798 §4):
//   Single NALU — raw NALU bytes, no wrapper
//   FU          — fragmentation units (type 49) for NALUs > MTU
//   AP          — aggregation packet (type 48) for small parameter-set bundles

#![cfg(feature = "hevc")]
// Phase 04 wires HevcPayloader into the HEVC video pipeline; until then the
// struct and helpers have no call site outside tests.
#![allow(dead_code)]

use bytes::{BufMut, Bytes, BytesMut};
// `webrtc` re-exports `rtp` at the crate root; no separate rtp dep needed.
// `rtp::error` is a private module — `Error` is re-exported but `Result` is not.
use webrtc::rtp::packetizer::Payloader;
type Result<T> = std::result::Result<T, webrtc::rtp::Error>;

// FU PayloadHdr: type=49, LayerId=0, TID=1 → (49<<9)|1 = 0x6201
const FU_HDR: [u8; 2] = [0x62, 0x01];
// AP PayloadHdr: type=48, LayerId=0, TID=1 → (48<<9)|1 = 0x6001
const AP_HDR: [u8; 2] = [0x60, 0x01];
// Overhead bytes consumed by an FU header (2-byte PayloadHdr + 1-byte FU byte).
const FU_OVERHEAD: usize = 3;

/// HEVC RTP payloader — RFC 7798 compliant.
#[derive(Debug, Clone, Default)]
pub struct HevcPayloader;

/// Split Annex-B bytestream into raw NALU slices (start codes stripped).
/// Handles both 3-byte and 4-byte start codes; prefers 4-byte to avoid
/// false matches on emulation-prevention sequences (`\x00\x00\x03`).
fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let n = data.len();

    // Skip leading start code.
    if n >= 4 && data[..4] == [0, 0, 0, 1] { start = 4; }
    else if n >= 3 && data[..3] == [0, 0, 1] { start = 3; }

    let mut i = start;
    while i + 2 < n {
        // Check 4-byte before 3-byte to never split on emulation-prevention `\x00\x00\x03\x01`.
        if i + 3 < n && data[i..i+4] == [0, 0, 0, 1] {
            if i > start { out.push(&data[start..i]); }
            start = i + 4; i = start; continue;
        }
        if data[i..i+3] == [0, 0, 1] {
            if i > start { out.push(&data[start..i]); }
            start = i + 3; i = start; continue;
        }
        i += 1;
    }
    if start < n { out.push(&data[start..]); }
    out
}

/// 6-bit NAL unit type from HEVC 2-byte header byte 0.
#[inline] fn nal_type(h: &[u8]) -> u8 { (h[0] >> 1) & 0x3F }

/// VPS=32, SPS=33, PPS=34.
#[inline] fn is_param_set(t: u8) -> bool { t == 32 || t == 33 || t == 34 }

/// Single-NALU RTP payload — raw NALU bytes (2-byte header already included).
#[inline] fn single_packet(nalu: &[u8]) -> Bytes { Bytes::copy_from_slice(nalu) }

/// Fragment a large NALU into FU RTP payloads.
/// Original 2-byte HEVC header is consumed; FuType carries the NAL type.
fn fu_packets(nalu: &[u8], mtu: usize) -> Vec<Bytes> {
    let nal_t = nal_type(nalu);
    let chunk = mtu.saturating_sub(FU_OVERHEAD);
    if chunk == 0 { return vec![]; }
    let body = &nalu[2..]; // skip original 2-byte header
    let mut pkts = Vec::with_capacity(body.len() / chunk + 1);
    let mut off = 0;
    while off < body.len() {
        let end = (off + chunk).min(body.len());
        let fu_byte = (if off == 0 { 0x80u8 } else { 0 })
            | (if end == body.len() { 0x40u8 } else { 0 })
            | nal_t;
        let mut p = BytesMut::with_capacity(FU_OVERHEAD + end - off);
        p.put_slice(&FU_HDR); p.put_u8(fu_byte); p.put_slice(&body[off..end]);
        pkts.push(p.freeze());
        off = end;
    }
    pkts
}

/// Build an AP RTP payload aggregating multiple NALUs (no DONL field).
/// Each NALU is prefixed by its `u16` big-endian byte count.
fn ap_packet(nalus: &[&[u8]]) -> Bytes {
    let cap = 2 + nalus.iter().map(|n| 2 + n.len()).sum::<usize>();
    let mut buf = BytesMut::with_capacity(cap);
    buf.put_slice(&AP_HDR);
    for n in nalus { buf.put_u16(n.len() as u16); buf.put_slice(n); }
    buf.freeze()
}

impl Payloader for HevcPayloader {
    /// Fragment an Annex-B HEVC sample into RTP payloads.
    fn payload(&mut self, mtu: usize, b: &Bytes) -> Result<Vec<Bytes>> {
        if b.is_empty() || mtu == 0 { return Ok(vec![]); }
        let nalus = split_annexb(b);
        let mut out: Vec<Bytes> = Vec::with_capacity(nalus.len());
        let mut i = 0;
        while i < nalus.len() {
            let nalu = nalus[i];
            // Greedy AP grouping: bundle consecutive small parameter-set NALUs
            // (VPS+SPS+PPS) into one AP packet to avoid 3 RTP packets per IDR.
            if nalu.len() >= 2 && is_param_set(nal_type(nalu)) && nalu.len() + 2 <= mtu {
                let mut grp: Vec<&[u8]> = vec![nalu];
                let mut used = 2 + 2 + nalu.len(); // AP hdr + first (size+nalu)
                let mut j = i + 1;
                while j < nalus.len() {
                    let nx = nalus[j];
                    let cost = 2 + nx.len();
                    if nx.len() >= 2 && is_param_set(nal_type(nx)) && used + cost <= mtu {
                        grp.push(nx); used += cost; j += 1;
                    } else { break; }
                }
                // Only wrap in AP if more than one NALU was collected.
                out.push(if grp.len() > 1 { ap_packet(&grp) } else { single_packet(nalu) });
                i = j; continue;
            }
            // Standard single/FU dispatch.
            if nalu.len() <= mtu { out.push(single_packet(nalu)); }
            else { out.extend(fu_packets(nalu, mtu)); }
            i += 1;
        }
        Ok(out)
    }

    fn clone_to(&self) -> Box<dyn Payloader + Send + Sync> { Box::new(self.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic NALU: 2-byte HEVC header + patterned body.
    fn make_nalu(nal_t: u8, len: usize) -> Vec<u8> {
        assert!(len >= 2);
        let mut v = vec![0u8; len];
        v[0] = nal_t << 1; v[1] = 0x01; // TID=1
        for (i, b) in v[2..].iter_mut().enumerate() { *b = (i & 0xFF) as u8; }
        v
    }

    // 1. split_annexb: mixed 4-byte and 3-byte start codes.
    #[test]
    fn test_split_annexb_mixed() {
        let nal1 = [0x62u8, 0x01, 0xAA];
        let nal2 = [0x42u8, 0x01, 0xBB];
        let mut data = vec![0, 0, 0, 1];
        data.extend_from_slice(&nal1);
        data.extend_from_slice(&[0, 0, 1]);
        data.extend_from_slice(&nal2);
        let nalus = split_annexb(&data);
        assert_eq!(nalus.len(), 2);
        assert_eq!(nalus[0], &nal1); assert_eq!(nalus[1], &nal2);
    }

    #[test] fn test_split_annexb_empty() { assert!(split_annexb(&[]).is_empty()); }

    // 2. Single NALU fits MTU → 1 packet matching NALU bytes exactly.
    #[test]
    fn test_single_nalu_fits_mtu() {
        let nalu = make_nalu(1, 100);
        let b = Bytes::from(nalu.clone());
        let pkts = HevcPayloader.payload(1400, &b).unwrap();
        assert_eq!(pkts.len(), 1);
        assert_eq!(&pkts[0][..], &nalu[..]);
    }

    // 3. Large IDR NALU (3000 bytes, mtu=1400) → correct FU fragmentation.
    #[test]
    fn test_fu_fragmentation() {
        let nalu = make_nalu(19, 3000); // IDR_W_RADL
        let chunk = 1400 - FU_OVERHEAD;
        let body_len = nalu.len() - 2;
        let expected = body_len.div_ceil(chunk);
        let pkts = fu_packets(&nalu, 1400);
        assert_eq!(pkts.len(), expected);

        let nt = nal_type(&nalu);
        // First: S=1, E=0.
        assert_eq!(&pkts[0][..2], &FU_HDR);
        assert_eq!(pkts[0][2] & 0x80, 0x80, "S bit on first FU");
        assert_eq!(pkts[0][2] & 0x40, 0x00, "no E on first FU");
        assert_eq!(pkts[0][2] & 0x3F, nt);
        // Last: S=0, E=1.
        let last = pkts.last().unwrap();
        assert_eq!(last[2] & 0x80, 0x00, "no S on last FU");
        assert_eq!(last[2] & 0x40, 0x40, "E bit on last FU");
        assert_eq!(last[2] & 0x3F, nt);
        // Middle: S=0, E=0.
        for p in &pkts[1..pkts.len()-1] { assert_eq!(p[2] & 0xC0, 0x00, "middle FU"); }
        // Reassembled data == nalu[2..].
        let reassembled: Vec<u8> = pkts.iter().flat_map(|p| p[FU_OVERHEAD..].iter().copied()).collect();
        assert_eq!(reassembled, &nalu[2..]);
    }

    // 4. ap_packet: correct PayloadHdr, size prefixes, and total length.
    #[test]
    fn test_ap_packet() {
        let vps = make_nalu(32, 20); let sps = make_nalu(33, 30); let pps = make_nalu(34, 40);
        let refs: &[&[u8]] = &[&vps, &sps, &pps];
        let pkt = ap_packet(refs);
        assert_eq!(&pkt[..2], &AP_HDR);
        let mut off = 2;
        for &n in refs {
            let sz = u16::from_be_bytes([pkt[off], pkt[off+1]]) as usize;
            assert_eq!(sz, n.len()); assert_eq!(&pkt[off+2..off+2+sz], n);
            off += 2 + sz;
        }
        assert_eq!(pkt.len(), 2 + (2+20) + (2+30) + (2+40));
    }

    // 4b. Payloader-level: VPS+SPS+PPS in one sample → single AP packet.
    #[test]
    fn test_payloader_aggregates_param_sets() {
        let vps = make_nalu(32, 20); let sps = make_nalu(33, 30); let pps = make_nalu(34, 40);
        let mut data = Vec::new();
        for n in [&vps, &sps, &pps] { data.extend_from_slice(&[0,0,0,1]); data.extend_from_slice(n); }
        let pkts = HevcPayloader.payload(1400, &Bytes::from(data)).unwrap();
        assert_eq!(pkts.len(), 1, "VPS+SPS+PPS → single AP");
        assert_eq!(&pkts[0][..2], &AP_HDR);
    }

    // 5. Empty input → empty output.
    #[test] fn test_empty_input() { assert!(HevcPayloader.payload(1400, &Bytes::new()).unwrap().is_empty()); }

    // 6. Single-byte NALU (degenerate) → single packet, no FU.
    #[test]
    fn test_single_byte_nalu() {
        // 3-byte SC + 1 byte payload.
        let data = Bytes::from_static(&[0x00, 0x00, 0x01, 0x26]);
        let pkts = HevcPayloader.payload(1400, &data).unwrap();
        assert_eq!(pkts.len(), 1); assert_eq!(&pkts[0][..], &[0x26u8]);
    }

    // 7. Emulation-prevention byte `\x00\x00\x03\x01` must NOT be treated as a start code.
    #[test]
    fn test_no_split_on_emulation_prevention() {
        let body = vec![0x00u8, 0x00, 0x03, 0x01, 0xAB, 0xCD];
        let mut data = vec![0, 0, 0, 1];
        data.extend_from_slice(&body);
        let nalus = split_annexb(&data);
        assert_eq!(nalus.len(), 1, "emulation-prevention must not split");
        assert_eq!(nalus[0], &body[..]);
    }
}
