//! Pipeline selection.
//!
//! Decides whether a desktop-streaming session runs over the legacy JPEG
//! DataChannel pipeline, the H.264 WebRTC-track pipeline, or the HEVC
//! WebRTC-track pipeline. Inputs:
//!
//! 1. **Operator preference** via the `OXI_VIDEO_PIPELINE` env var:
//!    - `auto` — HEVC → H.264 → JPEG in priority order, gated by feature
//!      compilation and client-advertised codec support.
//!      **The new default when the env var is unset.**
//!    - `hevc` — force HEVC; fail-closed if the client lacks decode (no
//!      silent fallback when the operator is explicit).
//!    - `h264` — force H.264; fail-closed if the client lacks decode (no
//!      silent JPEG fallback when the operator is explicit).
//!    - `jpeg` — force JPEG.
//!    - any other value or absent → `Auto`.
//!
//! 2. **Client capability** announced via `SignalIn::CapabilitiesClient`:
//!    - `webcodecs: true` OR the `codecs` list contains a baseline-3.1 H.264
//!      entry → client can decode H.264.
//!    - `codecs` list contains a string starting with `"h265"` or `"hevc"`
//!      (case-insensitive) → client can decode HEVC.
//!
//! 3. **Build-time features** `h264` and `hevc`: if the binary was built
//!    without either, `choose()` resolves to the next capable pipeline or
//!    JPEG. Forced selection of a missing feature returns an error rather
//!    than a silent fallback — operator intent is fail-closed.
//!
//! The resulting `(Pipeline, reason)` flows through the negotiation loop so
//! the server can attach the right track type before SDP exchange and ship
//! the reason in `SignalOut::Pipeline` for the SPA status pill tooltip.

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
    /// HEVC / H.265 via VideoToolbox (macOS) or platform encoder → RTP video track.
    #[cfg(feature = "hevc")]
    Hevc,
}

impl Pipeline {
    pub fn wire_name(self) -> &'static str {
        match self {
            Pipeline::Jpeg => "jpeg",
            #[cfg(feature = "h264")]
            Pipeline::H264 => "h264",
            // Wire name is "h265" to align with the SDP codec string family
            // and the SPA stats overlay label.
            #[cfg(feature = "hevc")]
            Pipeline::Hevc => "h265",
        }
    }
}

/// Operator's stated preference, distinct from the final `Pipeline` because
/// `Auto` resolves at session time using client capabilities. Forced values
/// (`H264`, `Hevc`, `Jpeg`) override client capability checks — a forced
/// pipeline with a client that can't decode is an explicit error, not a
/// silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorPref {
    Auto,
    #[cfg(feature = "h264")]
    H264,
    #[cfg(feature = "hevc")]
    Hevc,
    Jpeg,
}

/// Outcome of a `choose()` call. The reason string is the same identifier
/// the SPA status pill tooltip mapping consumes, so a downstream rename
/// requires updating exactly two surfaces (this file + the SPA mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub pipeline: Pipeline,
    pub reason: &'static str,
}

/// Error: operator forced H.264 but the client lacks decode. Fail-closed —
/// no silent JPEG fallback when the operator env var is explicit.
#[cfg_attr(not(feature = "h264"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedH264Unavailable {
    pub reason: &'static str,
}
impl std::fmt::Display for ForcedH264Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.reason) }
}
impl std::error::Error for ForcedH264Unavailable {}

/// Error: operator forced HEVC but the client lacks decode. Parallel to
/// `ForcedH264Unavailable` — no silent fallback to H.264 or JPEG.
#[cfg(feature = "hevc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedHevcUnavailable {
    pub reason: &'static str,
}
#[cfg(feature = "hevc")]
impl std::fmt::Display for ForcedHevcUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.reason) }
}
#[cfg(feature = "hevc")]
impl std::error::Error for ForcedHevcUnavailable {}

/// Read operator preference from `OXI_VIDEO_PIPELINE`. Unknown/absent → `Auto`.
/// Per-session override via `?force_pipeline=` uses `parse_force_pipeline`.
pub fn operator_preference() -> OperatorPref {
    parse_preference(env::var(ENV_VAR).ok().as_deref())
}

/// Strict parser for the `?force_pipeline=` query param — returns `None` for
/// unknown values so an SPA bug gets a 400, not a silent JPEG fallback.
pub fn parse_force_pipeline(value: &str) -> Option<OperatorPref> {
    match value {
        "auto" => Some(OperatorPref::Auto),
        #[cfg(feature = "h264")]
        "h264" => Some(OperatorPref::H264),
        #[cfg(not(feature = "h264"))]
        "h264" => None, // no encoder compiled
        #[cfg(feature = "hevc")]
        "hevc" => Some(OperatorPref::Hevc),
        #[cfg(not(feature = "hevc"))]
        "hevc" => None, // no encoder compiled
        "jpeg" => Some(OperatorPref::Jpeg),
        _ => None,
    }
}

/// Lenient parser for the `OXI_VIDEO_PIPELINE` env var — unknown → `Auto`
/// so an operator typo doesn't brick the daemon.
fn parse_preference(input: Option<&str>) -> OperatorPref {
    match input {
        Some("auto") => OperatorPref::Auto,
        #[cfg(feature = "h264")]
        Some("h264") => OperatorPref::H264,
        #[cfg(feature = "hevc")]
        Some("hevc") => OperatorPref::Hevc,
        Some("jpeg") => OperatorPref::Jpeg,
        _ => OperatorPref::Auto,
    }
}

/// Capabilities advertised by the client in `SignalIn::CapabilitiesClient`.
#[derive(Debug, Clone, Default)]
pub struct ClientCapabilities {
    pub codecs: Vec<String>,
    pub webcodecs: bool,
    /// Opts in to a receive-side `<audio>` sink for an Opus track.
    pub audio: bool,
    /// True when the client is on loopback; disables REMB-driven bitrate
    /// clamping (Chrome GCC collapses to ~5 kbps on loopback).
    pub loopback: bool,
}

impl ClientCapabilities {
    /// H.264 baseline-3.1: accept a literal codec match OR `webcodecs: true`
    /// (WebCodecs implies H.264 decode in all shipping browsers).
    #[cfg_attr(not(feature = "h264"), allow(dead_code))]
    pub fn supports_h264_baseline(&self) -> bool {
        if self.webcodecs { return true; }
        self.codecs.iter().any(|c| c.eq_ignore_ascii_case("h264-baseline-3.1") || c.starts_with("h264"))
    }

    /// HEVC / H.265: codec starts with `"h265"` or `"hevc"` (case-insensitive).
    /// No `webcodecs` shortcut — WebCodecs ≠ WebRTC HEVC; SPA must advertise
    /// explicitly.
    #[cfg_attr(not(feature = "hevc"), allow(dead_code))]
    pub fn supports_hevc(&self) -> bool {
        self.codecs.iter().any(|c| {
            let l = c.to_ascii_lowercase();
            l.starts_with("h265") || l.starts_with("hevc")
        })
    }
}

/// `true` when the binary includes the `h264` encoder.
pub const H264_COMPILED: bool = cfg!(feature = "h264");
/// `true` when the binary includes the `hevc` encoder.
#[allow(dead_code)]
pub const HEVC_COMPILED: bool = cfg!(feature = "hevc");

/// Select the pipeline for this session.
///
/// `Ok(Decision)` on success; `Err(boxed)` only when the operator forced a
/// pipeline (`OXI_VIDEO_PIPELINE` or `?force_pipeline=`) but the client lacks
/// decode support. The session layer converts the error into a WS close reason.
pub fn choose(
    operator: OperatorPref,
    client: &ClientCapabilities,
) -> Result<Decision, Box<dyn std::error::Error + Send + Sync>> {
    let _ = client; // unused on non-h264/non-hevc builds
    match operator {
        OperatorPref::Jpeg => Ok(Decision { pipeline: Pipeline::Jpeg, reason: "forced-jpeg" }),
        #[cfg(feature = "hevc")]
        OperatorPref::Hevc => {
            if client.supports_hevc() {
                Ok(Decision { pipeline: Pipeline::Hevc, reason: "forced-hevc" })
            } else {
                Err(Box::new(ForcedHevcUnavailable { reason: "forced-hevc-no-client" }))
            }
        }
        #[cfg(feature = "h264")]
        OperatorPref::H264 => {
            if client.supports_h264_baseline() {
                Ok(Decision { pipeline: Pipeline::H264, reason: "forced-h264" })
            } else {
                Err(Box::new(ForcedH264Unavailable { reason: "forced-h264-no-client" }))
            }
        }
        OperatorPref::Auto => {
            // Priority: HEVC → H.264 → JPEG. Each arm compiles only when the
            // matching feature is present.
            #[cfg(feature = "hevc")]
            if client.supports_hevc() {
                return Ok(Decision { pipeline: Pipeline::Hevc, reason: "auto-hevc" });
            }
            #[cfg(feature = "h264")]
            {
                if client.supports_h264_baseline() {
                    return Ok(Decision { pipeline: Pipeline::H264, reason: "auto-h264" });
                }
                Ok(Decision { pipeline: Pipeline::Jpeg, reason: "auto-jpeg-no-client" })
            }
            #[cfg(not(feature = "h264"))]
            Ok(Decision { pipeline: Pipeline::Jpeg, reason: "auto-jpeg-no-feature" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(codecs: &[&str], webcodecs: bool) -> ClientCapabilities {
        ClientCapabilities {
            codecs: codecs.iter().map(|s| s.to_string()).collect(),
            webcodecs,
            ..Default::default()
        }
    }

    #[test]
    fn parse_force_pipeline_known_values() {
        assert_eq!(parse_force_pipeline("auto"), Some(OperatorPref::Auto));
        assert_eq!(parse_force_pipeline("jpeg"), Some(OperatorPref::Jpeg));
        #[cfg(feature = "h264")]
        assert_eq!(parse_force_pipeline("h264"), Some(OperatorPref::H264));
        assert_eq!(parse_force_pipeline("HEVC"), None);
        assert_eq!(parse_force_pipeline(""), None);
    }

    #[test]
    fn parse_preference_env_is_lenient() {
        assert_eq!(parse_preference(None), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("")), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("nope")), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("jpeg")), OperatorPref::Jpeg);
        #[cfg(feature = "h264")]
        assert_eq!(parse_preference(Some("h264")), OperatorPref::H264);
    }

    #[test]
    fn auto_with_capable_client_picks_h264_when_compiled() {
        let d = choose(OperatorPref::Auto, &caps(&[], true)).expect("auto never errors");
        #[cfg(all(feature = "h264", not(feature = "hevc")))]
        { assert_eq!(d.pipeline, Pipeline::H264); assert_eq!(d.reason, "auto-h264"); }
        #[cfg(not(feature = "h264"))]
        { assert_eq!(d.pipeline, Pipeline::Jpeg); assert_eq!(d.reason, "auto-jpeg-no-feature"); }
        let _ = d;
    }

    #[test]
    fn auto_with_incapable_client_picks_jpeg() {
        let d = choose(OperatorPref::Auto, &caps(&[], false)).expect("auto never errors");
        assert_eq!(d.pipeline, Pipeline::Jpeg);
        #[cfg(feature = "h264")] assert_eq!(d.reason, "auto-jpeg-no-client");
        #[cfg(not(feature = "h264"))] assert_eq!(d.reason, "auto-jpeg-no-feature");
    }

    #[test]
    fn forced_jpeg_is_always_jpeg() {
        let d = choose(OperatorPref::Jpeg, &caps(&[], true)).unwrap();
        assert_eq!(d.pipeline, Pipeline::Jpeg);
        assert_eq!(d.reason, "forced-jpeg");
    }

    #[cfg(feature = "h264")]
    #[test]
    fn forced_h264_with_capable_client_picks_h264() {
        let d = choose(OperatorPref::H264, &caps(&[], true)).unwrap();
        assert_eq!(d.pipeline, Pipeline::H264);
        assert_eq!(d.reason, "forced-h264");
    }

    #[cfg(feature = "h264")]
    #[test]
    fn forced_h264_without_client_returns_error() {
        let e = choose(OperatorPref::H264, &caps(&[], false))
            .expect_err("fail-closed on forced h264 + incapable client");
        assert_eq!(e.to_string(), "forced-h264-no-client");
    }

    #[test]
    fn codec_list_h264_matches_case_insensitively() {
        assert!(caps(&["H264-Baseline-3.1"], false).supports_h264_baseline());
    }

    #[test]
    fn wire_name_matches_protocol() {
        assert_eq!(Pipeline::Jpeg.wire_name(), "jpeg");
        #[cfg(feature = "h264")] assert_eq!(Pipeline::H264.wire_name(), "h264");
        #[cfg(feature = "hevc")] assert_eq!(Pipeline::Hevc.wire_name(), "h265");
    }

    // HEVC tests — compiled only when hevc feature is active.

    #[cfg(feature = "hevc")]
    #[test]
    fn forced_hevc_with_capable_client_picks_hevc() {
        let d = choose(OperatorPref::Hevc, &caps(&["h265-main-5.0"], false)).unwrap();
        assert_eq!(d.pipeline, Pipeline::Hevc);
        assert_eq!(d.reason, "forced-hevc");
    }

    #[cfg(feature = "hevc")]
    #[test]
    fn forced_hevc_with_incapable_client_errors() {
        let e = choose(OperatorPref::Hevc, &caps(&[], false))
            .expect_err("fail-closed on forced hevc + incapable client");
        assert_eq!(e.to_string(), "forced-hevc-no-client");
    }

    #[cfg(feature = "hevc")]
    #[test]
    fn auto_with_hevc_capable_client_picks_hevc() {
        let d = choose(OperatorPref::Auto, &caps(&["h265-main-5.0"], false))
            .expect("auto never errors");
        assert_eq!(d.pipeline, Pipeline::Hevc);
        assert_eq!(d.reason, "auto-hevc");
    }

    #[cfg(feature = "hevc")]
    #[test]
    fn auto_with_h264_only_client_picks_h264() {
        // Client advertises h264 but not hevc; HEVC must not be picked.
        let d = choose(OperatorPref::Auto, &caps(&["h264-baseline-3.1"], false))
            .expect("auto never errors");
        #[cfg(feature = "h264")]
        { assert_eq!(d.pipeline, Pipeline::H264); assert_eq!(d.reason, "auto-h264"); }
        #[cfg(not(feature = "h264"))]
        assert_eq!(d.pipeline, Pipeline::Jpeg);
        let _ = d;
    }

    #[cfg(feature = "hevc")]
    #[test]
    fn parse_force_pipeline_hevc_returns_some_when_feature() {
        assert_eq!(parse_force_pipeline("hevc"), Some(OperatorPref::Hevc));
    }

    #[cfg(not(feature = "hevc"))]
    #[test]
    fn parse_force_pipeline_hevc_returns_none_when_no_feature() {
        assert_eq!(parse_force_pipeline("hevc"), None);
    }

    #[cfg(feature = "hevc")]
    #[test]
    fn wire_name_hevc() {
        assert_eq!(Pipeline::Hevc.wire_name(), "h265");
    }

    #[test]
    fn supports_hevc_case_insensitive() {
        assert!(caps(&["h265-main-5.0"], false).supports_hevc(), "h265-main-5.0 must match");
        assert!(caps(&["HEVC"], false).supports_hevc(), "HEVC must match case-insensitively");
        assert!(!caps(&["h264"], false).supports_hevc(), "h264 must not match hevc check");
    }
}
