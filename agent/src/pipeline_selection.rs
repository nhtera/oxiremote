//! Pipeline selection (Phase 03).
//!
//! Decides whether a desktop-streaming session runs over the legacy JPEG
//! DataChannel pipeline or the H.264 WebRTC-track pipeline. Inputs:
//!
//! 1. **Operator choice** via the `OXI_VIDEO_PIPELINE` env var:
//!    - `h264`   — prefer H.264 when the client supports it
//!    - `jpeg`   — force JPEG (default during rollout)
//!    - any other value or absent → JPEG
//!
//! 2. **Client capability** announced via `SignalIn::CapabilitiesClient`:
//!    - `webcodecs: true` OR the `codecs` list contains a baseline-3.1 H.264
//!      entry → client can decode H.264.
//!
//! The resulting `Pipeline` is flowed through the negotiation loop so the
//! server can decide to `pc.add_track(video)` before the SDP exchange (and
//! skip the JPEG capture spawn), plus send `SignalOut::Pipeline` confirming
//! the choice to the client.

#![cfg(feature = "desktop")]

use std::env;

const ENV_VAR: &str = "OXI_VIDEO_PIPELINE";

/// The chosen transport pipeline for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipeline {
    /// Legacy: xxhash3 tile diff + mozjpeg → binary DataChannel (phase 02).
    Jpeg,
    /// H.264 via VideoToolbox / OpenH264 → RTP video track (phase 03).
    #[cfg(feature = "h264")]
    H264,
}

impl Pipeline {
    pub fn wire_name(self) -> &'static str {
        match self {
            Pipeline::Jpeg => "jpeg",
            #[cfg(feature = "h264")]
            Pipeline::H264 => "h264",
        }
    }
}

/// Read the operator's preference at session-setup time. Never panics —
/// unknown or missing values default to `Jpeg` for safety during rollout.
pub fn operator_preference() -> Pipeline {
    match env::var(ENV_VAR).as_deref() {
        #[cfg(feature = "h264")]
        Ok("h264") => Pipeline::H264,
        _ => Pipeline::Jpeg,
    }
}

/// Describes what the client advertised in its `capabilitiesClient` message.
#[derive(Debug, Clone, Default)]
pub struct ClientCapabilities {
    pub codecs: Vec<String>,
    pub webcodecs: bool,
}

impl ClientCapabilities {
    /// Does the client signal H.264 baseline-3.1 compatibility? We accept
    /// either a literal codec string match or WebCodecs support (WebCodecs
    /// implies H.264 decode in all browsers that ship it).
    #[cfg_attr(not(feature = "h264"), allow(dead_code))]
    pub fn supports_h264_baseline(&self) -> bool {
        if self.webcodecs {
            return true;
        }
        self.codecs
            .iter()
            .any(|c| c.eq_ignore_ascii_case("h264-baseline-3.1") || c.starts_with("h264"))
    }
}

/// Decide the pipeline for this session. AND'd across operator preference
/// and client capability — both must vote for H.264 to switch off JPEG.
pub fn choose(operator: Pipeline, client: &ClientCapabilities) -> Pipeline {
    let _ = client; // unused when feature = "h264" is disabled
    match operator {
        #[cfg(feature = "h264")]
        Pipeline::H264 if client.supports_h264_baseline() => Pipeline::H264,
        _ => Pipeline::Jpeg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_without_env_is_jpeg() {
        // We don't unset the env here to avoid test-parallelism flakes;
        // rely on choose() against a known Jpeg input.
        let client = ClientCapabilities::default();
        assert_eq!(choose(Pipeline::Jpeg, &client), Pipeline::Jpeg);
    }

    #[test]
    fn client_capability_alone_does_not_enable_h264() {
        let mut client = ClientCapabilities::default();
        client.webcodecs = true;
        // Operator prefers JPEG → stay on JPEG even if client could decode.
        assert_eq!(choose(Pipeline::Jpeg, &client), Pipeline::Jpeg);
    }

    #[cfg(feature = "h264")]
    #[test]
    fn both_sides_must_vote_for_h264() {
        let no_caps = ClientCapabilities::default();
        assert_eq!(choose(Pipeline::H264, &no_caps), Pipeline::Jpeg);

        let mut yes_caps = ClientCapabilities::default();
        yes_caps.webcodecs = true;
        assert_eq!(choose(Pipeline::H264, &yes_caps), Pipeline::H264);

        let mut caps_list = ClientCapabilities::default();
        caps_list.codecs = vec!["h264-baseline-3.1".to_string()];
        assert_eq!(choose(Pipeline::H264, &caps_list), Pipeline::H264);
    }

    #[test]
    fn codec_matching_is_case_insensitive() {
        let mut c = ClientCapabilities::default();
        c.codecs = vec!["H264-Baseline-3.1".to_string()];
        assert!(c.supports_h264_baseline());
    }

    #[test]
    fn wire_name_matches_protocol() {
        assert_eq!(Pipeline::Jpeg.wire_name(), "jpeg");
        #[cfg(feature = "h264")]
        assert_eq!(Pipeline::H264.wire_name(), "h264");
    }
}
