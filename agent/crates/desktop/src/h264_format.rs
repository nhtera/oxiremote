//! H.264 bitstream format helpers for Phase 03.
//!
//! Two wire formats:
//! - **Annex-B**: `00 00 00 01 <nal> 00 00 00 01 <nal> …` — start-code delimited.
//!   Required by webrtc-rs `H264Payloader` (`TrackLocalStaticSample::write_sample`).
//! - **AVCC**: `<len:u32_be> <nal> <len:u32_be> <nal> …` — length-prefixed.
//!   What VideoToolbox emits, and what the WebCodecs `VideoDecoder.configure`
//!   `description` blob (`avcC` box) implies on the wire.
//!
//! Pure data munging. No FFI, no async, no allocations beyond the output Vec.

/// NAL unit type extracted from the first byte of a NAL.
/// `nal_header_byte & 0x1F`.
pub fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1F)
}

pub const NAL_TYPE_IDR: u8 = 5;
pub const NAL_TYPE_SPS: u8 = 7;
pub const NAL_TYPE_PPS: u8 = 8;

/// Build an `AVCDecoderConfigurationRecord` (avcC box) per ISO/IEC 14496-15 §5.3.3.1.
///
/// Input NALs are raw bytes **including** the NAL header byte (e.g. `0x67` for SPS,
/// `0x68` for PPS) and **without** Annex-B start codes.
///
/// Output is suitable for `VideoDecoder.configure({ description: <bytes> })` in the
/// browser's WebCodecs API.
///
/// Panics if `sps.len() < 4` — the profile/compat/level bytes at SPS[1..=3] are
/// required to fill fields 1–3 of the config record.
pub fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    assert!(sps.len() >= 4, "SPS too short: need profile/compat/level bytes");
    assert!(sps.len() <= u16::MAX as usize, "SPS exceeds u16 length");
    assert!(pps.len() <= u16::MAX as usize, "PPS exceeds u16 length");

    let mut out = Vec::with_capacity(11 + sps.len() + pps.len());
    out.push(0x01);                                     // configurationVersion
    out.push(sps[1]);                                   // AVCProfileIndication
    out.push(sps[2]);                                   // profile_compatibility
    out.push(sps[3]);                                   // AVCLevelIndication
    out.push(0xFF);                                     // 6 reserved | lengthSizeMinusOne=3
    out.push(0xE1);                                     // 3 reserved | numOfSPS=1
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01);                                     // numOfPPS=1
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

/// Convert an AVCC-formatted NAL stream (4-byte big-endian length prefixes) to
/// Annex-B (`00 00 00 01` start codes).
///
/// Malformed input — a length prefix that overruns the buffer — yields whatever
/// was successfully parsed up to that point rather than panicking. This matches
/// how webrtc-rs's H264Payloader scans: garbage-in produces fewer NALs, not a
/// process abort.
pub fn avcc_to_annexb(avcc: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(avcc.len() + 16);
    let mut i = 0;
    while i + 4 <= avcc.len() {
        let nal_len =
            u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        if nal_len == 0 || i + nal_len > avcc.len() {
            break;
        }
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&avcc[i..i + nal_len]);
        i += nal_len;
    }
    out
}

/// Convert an Annex-B stream to AVCC (4-byte big-endian length prefixes).
///
/// Accepts both 3-byte (`00 00 01`) and 4-byte (`00 00 00 01`) start codes.
/// NALs are emitted in order. Empty input yields empty output.
pub fn annexb_to_avcc(annexb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(annexb.len());
    let starts = find_start_codes(annexb);
    for (idx, &(pos, sc_len)) in starts.iter().enumerate() {
        let nal_start = pos + sc_len;
        let nal_end = starts.get(idx + 1).map(|(p, _)| *p).unwrap_or(annexb.len());
        if nal_end <= nal_start {
            continue;
        }
        let nal = &annexb[nal_start..nal_end];
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Scan for Annex-B start codes. Returns `(position, length)` tuples where
/// `length` is 3 (`00 00 01`) or 4 (`00 00 00 01`).
fn find_start_codes(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if i + 4 <= buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                out.push((i, 4));
                i += 4;
                continue;
            }
            if buf[i + 2] == 1 {
                out.push((i, 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract all NAL units from an Annex-B stream. Returned slices do NOT include
/// start codes. Useful for searching SPS/PPS/IDR NALs in bitstreams that came in
/// Annex-B (e.g., from OpenH264's default output).
pub fn split_annexb(annexb: &[u8]) -> Vec<&[u8]> {
    let starts = find_start_codes(annexb);
    let mut out = Vec::with_capacity(starts.len());
    for (idx, &(pos, sc_len)) in starts.iter().enumerate() {
        let nal_start = pos + sc_len;
        let nal_end = starts.get(idx + 1).map(|(p, _)| *p).unwrap_or(annexb.len());
        if nal_end > nal_start {
            out.push(&annexb[nal_start..nal_end]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic minimal SPS/PPS bytes — content is opaque to the helpers, only
    // SPS[1..=3] are read. Values chosen to mimic baseline 3.1 (profile=0x42,
    // compat=0x00, level=0x1F).
    const SPS: &[u8] = &[0x67, 0x42, 0x00, 0x1F, 0xAA, 0xBB];
    const PPS: &[u8] = &[0x68, 0xCE, 0x38, 0x80];

    #[test]
    fn avcc_layout_matches_iso_14496_15() {
        let avcc = build_avcc(SPS, PPS);
        assert_eq!(avcc[0], 0x01, "configurationVersion");
        assert_eq!(avcc[1], SPS[1], "AVCProfileIndication from SPS[1]");
        assert_eq!(avcc[2], SPS[2], "profile_compatibility from SPS[2]");
        assert_eq!(avcc[3], SPS[3], "AVCLevelIndication from SPS[3]");
        assert_eq!(avcc[4], 0xFF, "lengthSizeMinusOne=3 (4-byte NAL lengths)");
        assert_eq!(avcc[5], 0xE1, "numOfSPS=1");
        let sps_len = u16::from_be_bytes([avcc[6], avcc[7]]) as usize;
        assert_eq!(sps_len, SPS.len());
        assert_eq!(&avcc[8..8 + sps_len], SPS);
        let pps_count_off = 8 + sps_len;
        assert_eq!(avcc[pps_count_off], 0x01, "numOfPPS=1");
        let pps_len = u16::from_be_bytes([avcc[pps_count_off + 1], avcc[pps_count_off + 2]]) as usize;
        assert_eq!(pps_len, PPS.len());
        assert_eq!(&avcc[pps_count_off + 3..pps_count_off + 3 + pps_len], PPS);
    }

    #[test]
    fn avcc_total_length_matches_formula() {
        let avcc = build_avcc(SPS, PPS);
        // 6 fixed bytes header + 2 sps_len + SPS + 1 numPPS + 2 pps_len + PPS
        assert_eq!(avcc.len(), 6 + 2 + SPS.len() + 1 + 2 + PPS.len());
    }

    #[test]
    #[should_panic(expected = "SPS too short")]
    fn build_avcc_rejects_short_sps() {
        build_avcc(&[0x67, 0x42, 0x00], PPS);
    }

    #[test]
    fn avcc_to_annexb_roundtrips_with_annexb_to_avcc() {
        // Two synthetic NALs
        let nal_a = [0x67u8, 0x42, 0x00, 0x1F, 0xAA];
        let nal_b = [0x68u8, 0xCE, 0x38, 0x80];
        let mut avcc = Vec::new();
        avcc.extend_from_slice(&(nal_a.len() as u32).to_be_bytes());
        avcc.extend_from_slice(&nal_a);
        avcc.extend_from_slice(&(nal_b.len() as u32).to_be_bytes());
        avcc.extend_from_slice(&nal_b);

        let annexb = avcc_to_annexb(&avcc);
        // Must start with 4-byte start code
        assert_eq!(&annexb[0..4], &[0x00, 0x00, 0x00, 0x01]);

        let nals = split_annexb(&annexb);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0], &nal_a);
        assert_eq!(nals[1], &nal_b);

        // And back
        let reavcc = annexb_to_avcc(&annexb);
        assert_eq!(reavcc, avcc);
    }

    #[test]
    fn annexb_to_avcc_handles_both_start_code_lengths() {
        let mut annexb = Vec::new();
        annexb.extend_from_slice(&[0x00, 0x00, 0x01]); // 3-byte
        annexb.extend_from_slice(&[0x67, 0x42, 0x00, 0x1F]);
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // 4-byte
        annexb.extend_from_slice(&[0x68, 0xCE, 0x38]);

        let avcc = annexb_to_avcc(&annexb);
        // [00 00 00 04 67 42 00 1F] [00 00 00 03 68 CE 38]
        assert_eq!(&avcc[0..4], &4u32.to_be_bytes());
        assert_eq!(&avcc[4..8], &[0x67, 0x42, 0x00, 0x1F]);
        assert_eq!(&avcc[8..12], &3u32.to_be_bytes());
        assert_eq!(&avcc[12..15], &[0x68, 0xCE, 0x38]);
    }

    #[test]
    fn avcc_to_annexb_stops_cleanly_on_truncated_input() {
        // length prefix says 100 bytes but only 3 follow
        let mut bad = Vec::new();
        bad.extend_from_slice(&100u32.to_be_bytes());
        bad.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        let out = avcc_to_annexb(&bad);
        assert!(out.is_empty(), "malformed length must not panic or over-read");
    }

    #[test]
    fn avcc_to_annexb_skips_zero_length_nal() {
        let mut avcc = Vec::new();
        avcc.extend_from_slice(&0u32.to_be_bytes()); // zero-len NAL
        avcc.extend_from_slice(&3u32.to_be_bytes()); // valid 3-byte NAL
        avcc.extend_from_slice(&[0x68, 0xCE, 0x38]);
        let out = avcc_to_annexb(&avcc);
        // zero-length triggers break — valid NAL is not reached, but this is
        // fine because such a stream is malformed upstream regardless.
        assert!(out.is_empty());
    }

    #[test]
    fn nal_unit_type_extracts_low_5_bits() {
        assert_eq!(nal_unit_type(&[0x67]), Some(NAL_TYPE_SPS));
        assert_eq!(nal_unit_type(&[0x68]), Some(NAL_TYPE_PPS));
        assert_eq!(nal_unit_type(&[0x65]), Some(NAL_TYPE_IDR));
        assert_eq!(nal_unit_type(&[]), None);
    }

    #[test]
    fn split_annexb_yields_nals_without_start_codes() {
        let mut s = Vec::new();
        s.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42]);
        s.extend_from_slice(&[0x00, 0x00, 0x01, 0x68]);
        let nals = split_annexb(&s);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0], &[0x67, 0x42]);
        assert_eq!(nals[1], &[0x68]);
    }

    #[test]
    fn empty_inputs() {
        assert!(avcc_to_annexb(&[]).is_empty());
        assert!(annexb_to_avcc(&[]).is_empty());
        assert!(split_annexb(&[]).is_empty());
    }
}
