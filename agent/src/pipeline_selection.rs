//! Pipeline selection.
//!
//! Decides which encoder transport carries a desktop-streaming session:
//! AV1 → VP9 → H.264 → JPEG. Inputs:
//!
//! 1. **Operator preference** via the `OXI_VIDEO_PIPELINE` env var:
//!    - `auto` — AV1 → VP9 → H.264 → JPEG in priority order, gated
//!      by feature compilation and client-advertised codec support.
//!      **The new default when the env var is unset.**
//!    - `av1` — force AV1; fail-closed if the client lacks decode (no
//!      silent fallback when the operator is explicit). Safari ≤ 18 has
//!      no AV1 WebRTC support and will hit this branch.
//!    - `vp9` — force VP9; fail-closed if the client lacks decode.
//!    - `h264` — force H.264; fail-closed if the client lacks decode.
//!    - `jpeg` — force JPEG.
//!    - any other value or absent → `Auto`.
//!
//! 2. **Client capability** announced via `SignalIn::CapabilitiesClient`:
//!    - `webcodecs: true` OR the `codecs` list contains a baseline-3.1 H.264
//!      entry → client can decode H.264.
//!    - `codecs` list contains a string starting with `"vp9"`
//!      (case-insensitive) → client can decode VP9.
//!    - `codecs` list contains a string starting with `"av1"` or `"av01"`
//!      (case-insensitive) → client can decode AV1. Browsers expose both
//!      forms; Chrome canonical is `"AV1"` from `RTCRtpReceiver`, while
//!      WebCodecs and Media Capabilities use the `"av01.*"` MIME suffix.
//!
//! 3. **Build-time features** `h264`, `vp9`, `av1`: if the binary was built
//!    without one, `choose()` resolves to the next capable pipeline or JPEG.
//!    Forced selection of a missing feature returns an error rather than a
//!    silent fallback — operator intent is fail-closed.
//!
//! The resulting `(Pipeline, reason)` flows through the negotiation loop
//! so the server can attach the right track type before SDP exchange and
//! ship the reason in `SignalOut::Pipeline` for the SPA status pill.

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
    /// VP9 via libvpx (screen-content tuned, active-map driven) → RTP video track.
    #[cfg(feature = "vp9")]
    Vp9,
    /// AV1 via libaom (AOM_CONTENT_SCREEN + palette + IBC) → RTP video track.
    #[cfg(feature = "av1")]
    Av1,
}

impl Pipeline {
    pub fn wire_name(self) -> &'static str {
        match self {
            Pipeline::Jpeg => "jpeg",
            #[cfg(feature = "h264")]
            Pipeline::H264 => "h264",
            #[cfg(feature = "vp9")]
            Pipeline::Vp9 => "vp9",
            #[cfg(feature = "av1")]
            Pipeline::Av1 => "av1",
        }
    }
}

/// Operator's stated preference, distinct from the final `Pipeline` because
/// `Auto` resolves at session time using client capabilities. Forced values
/// override client capability checks — a forced pipeline with a client that
/// can't decode is an explicit error, not a silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorPref {
    Auto,
    #[cfg(feature = "h264")]
    H264,
    #[cfg(feature = "vp9")]
    Vp9,
    #[cfg(feature = "av1")]
    Av1,
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

/// Error: operator forced VP9 but the client lacks decode.
#[cfg(feature = "vp9")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedVp9Unavailable {
    pub reason: &'static str,
}
#[cfg(feature = "vp9")]
impl std::fmt::Display for ForcedVp9Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.reason) }
}
#[cfg(feature = "vp9")]
impl std::error::Error for ForcedVp9Unavailable {}

/// Error: operator forced AV1 but the client lacks decode. Safari ≤ 18
/// hits this branch — Apple has stated no plans for AV1 WebRTC.
#[cfg(feature = "av1")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedAv1Unavailable {
    pub reason: &'static str,
}
#[cfg(feature = "av1")]
impl std::fmt::Display for ForcedAv1Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.reason) }
}
#[cfg(feature = "av1")]
impl std::error::Error for ForcedAv1Unavailable {}

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
        #[cfg(feature = "vp9")]
        "vp9" => Some(OperatorPref::Vp9),
        #[cfg(not(feature = "vp9"))]
        "vp9" => None, // no encoder compiled
        #[cfg(feature = "av1")]
        "av1" => Some(OperatorPref::Av1),
        #[cfg(not(feature = "av1"))]
        "av1" => None, // no encoder compiled
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
        #[cfg(feature = "vp9")]
        Some("vp9") => OperatorPref::Vp9,
        #[cfg(feature = "av1")]
        Some("av1") => OperatorPref::Av1,
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

    /// VP9: codec starts with `"vp9"` (case-insensitive). Includes
    /// profile-specific strings like `"vp9-profile0"` that future phases
    /// may emit. No `webcodecs` shortcut — VP9 must be explicitly
    /// advertised because Safari ≤ 16 lacks VP9 WebRTC support.
    #[cfg_attr(not(feature = "vp9"), allow(dead_code))]
    pub fn supports_vp9(&self) -> bool {
        self.codecs.iter().any(|c| c.to_ascii_lowercase().starts_with("vp9"))
    }

    /// AV1: codec starts with `"av1"` (canonical from `RTCRtpReceiver`) or
    /// `"av01"` (MIME-style from Media Capabilities). Both forms accepted
    /// case-insensitive. No `webcodecs` shortcut — Safari WebCodecs may
    /// expose AV1 decode while WebRTC does not.
    #[cfg_attr(not(feature = "av1"), allow(dead_code))]
    pub fn supports_av1(&self) -> bool {
        self.codecs.iter().any(|c| {
            let l = c.to_ascii_lowercase();
            l.starts_with("av1") || l.starts_with("av01")
        })
    }
}

/// `true` when the binary includes the `h264` encoder.
pub const H264_COMPILED: bool = cfg!(feature = "h264");
/// `true` when the binary includes the `vp9` encoder.
#[allow(dead_code)]
pub const VP9_COMPILED: bool = cfg!(feature = "vp9");
/// `true` when the binary includes the `av1` encoder.
#[allow(dead_code)]
pub const AV1_COMPILED: bool = cfg!(feature = "av1");

/// Select the pipeline for this session.
///
/// `Ok(Decision)` on success; `Err(boxed)` only when the operator forced a
/// pipeline (`OXI_VIDEO_PIPELINE` or `?force_pipeline=`) but the client lacks
/// decode support. The session layer converts the error into a WS close reason.
///
/// Auto priority chain: **AV1 → VP9 → H.264 → JPEG**. Matches Chrome
/// Remote Desktop's default (best quality-per-bit first). Each arm compiles
/// only when the matching feature is present, so a JPEG-only build collapses
/// the chain to `Pipeline::Jpeg`.
pub fn choose(
    operator: OperatorPref,
    client: &ClientCapabilities,
) -> Result<Decision, Box<dyn std::error::Error + Send + Sync>> {
    let _ = client; // unused on non-codec builds
    match operator {
        OperatorPref::Jpeg => Ok(Decision { pipeline: Pipeline::Jpeg, reason: "forced-jpeg" }),
        #[cfg(feature = "av1")]
        OperatorPref::Av1 => {
            if client.supports_av1() {
                Ok(Decision { pipeline: Pipeline::Av1, reason: "forced-av1" })
            } else {
                Err(Box::new(ForcedAv1Unavailable { reason: "forced-av1-no-client" }))
            }
        }
        #[cfg(feature = "vp9")]
        OperatorPref::Vp9 => {
            if client.supports_vp9() {
                Ok(Decision { pipeline: Pipeline::Vp9, reason: "forced-vp9" })
            } else {
                Err(Box::new(ForcedVp9Unavailable { reason: "forced-vp9-no-client" }))
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
            // Priority: AV1 → VP9 → H.264 → JPEG. Each arm compiles only
            // when the matching feature is present.
            #[cfg(feature = "av1")]
            if client.supports_av1() {
                return Ok(Decision { pipeline: Pipeline::Av1, reason: "auto-av1" });
            }
            #[cfg(feature = "vp9")]
            if client.supports_vp9() {
                return Ok(Decision { pipeline: Pipeline::Vp9, reason: "auto-vp9" });
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
        #[cfg(feature = "vp9")]
        assert_eq!(parse_force_pipeline("vp9"), Some(OperatorPref::Vp9));
        #[cfg(feature = "av1")]
        assert_eq!(parse_force_pipeline("av1"), Some(OperatorPref::Av1));
        assert_eq!(parse_force_pipeline("hevc"), None);
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
        #[cfg(feature = "vp9")]
        assert_eq!(parse_preference(Some("vp9")), OperatorPref::Vp9);
        #[cfg(feature = "av1")]
        assert_eq!(parse_preference(Some("av1")), OperatorPref::Av1);
    }

    #[test]
    fn auto_with_capable_client_picks_highest_priority() {
        let d = choose(OperatorPref::Auto, &caps(&[], true)).expect("auto never errors");
        // Highest-priority compiled codec wins for the H.264-only baseline case.
        #[cfg(all(feature = "h264", not(feature = "vp9"), not(feature = "av1")))]
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
        #[cfg(feature = "vp9")] assert_eq!(Pipeline::Vp9.wire_name(), "vp9");
        #[cfg(feature = "av1")] assert_eq!(Pipeline::Av1.wire_name(), "av1");
    }

    // VP9 tests — compiled only when vp9 feature is active.

    #[cfg(feature = "vp9")]
    #[test]
    fn forced_vp9_with_capable_client_picks_vp9() {
        let d = choose(OperatorPref::Vp9, &caps(&["vp9"], false)).unwrap();
        assert_eq!(d.pipeline, Pipeline::Vp9);
        assert_eq!(d.reason, "forced-vp9");
    }

    #[cfg(feature = "vp9")]
    #[test]
    fn forced_vp9_without_client_returns_error() {
        let e = choose(OperatorPref::Vp9, &caps(&[], false))
            .expect_err("fail-closed on forced vp9 + incapable client");
        assert_eq!(e.to_string(), "forced-vp9-no-client");
    }

    #[cfg(feature = "vp9")]
    #[test]
    fn auto_with_vp9_capable_client_picks_vp9_when_no_av1() {
        // Client advertises VP9 but not AV1 — VP9 must win the auto chain.
        let d = choose(OperatorPref::Auto, &caps(&["vp9"], false))
            .expect("auto never errors");
        assert_eq!(d.pipeline, Pipeline::Vp9);
        assert_eq!(d.reason, "auto-vp9");
    }

    #[cfg(feature = "vp9")]
    #[test]
    fn parse_force_pipeline_vp9_returns_some_when_feature() {
        assert_eq!(parse_force_pipeline("vp9"), Some(OperatorPref::Vp9));
    }

    #[cfg(not(feature = "vp9"))]
    #[test]
    fn parse_force_pipeline_vp9_returns_none_when_no_feature() {
        assert_eq!(parse_force_pipeline("vp9"), None);
    }

    #[cfg(feature = "vp9")]
    #[test]
    fn wire_name_vp9() {
        assert_eq!(Pipeline::Vp9.wire_name(), "vp9");
    }

    #[test]
    fn supports_vp9_case_insensitive() {
        let mk = |codec: &str| ClientCapabilities {
            codecs: vec![codec.into()],
            ..Default::default()
        };
        assert!(mk("vp9").supports_vp9(), "vp9 must match");
        assert!(mk("VP9-profile0").supports_vp9(), "VP9-profile0 must match case-insensitively");
        assert!(!mk("h264-baseline-3.1").supports_vp9(), "h264 must not match vp9 check");
    }

    // AV1 tests — compiled only when av1 feature is active.

    #[cfg(feature = "av1")]
    #[test]
    fn forced_av1_with_capable_client_picks_av1() {
        let d = choose(OperatorPref::Av1, &caps(&["av1"], false)).unwrap();
        assert_eq!(d.pipeline, Pipeline::Av1);
        assert_eq!(d.reason, "forced-av1");
    }

    #[cfg(feature = "av1")]
    #[test]
    fn forced_av1_with_av01_form_works() {
        // Browsers may expose AV1 via the av01.* MIME family.
        let d = choose(OperatorPref::Av1, &caps(&["av01.0.08M.08"], false)).unwrap();
        assert_eq!(d.pipeline, Pipeline::Av1);
    }

    #[cfg(feature = "av1")]
    #[test]
    fn forced_av1_without_client_returns_error() {
        let e = choose(OperatorPref::Av1, &caps(&[], false))
            .expect_err("fail-closed on forced av1 + incapable client (Safari hits this)");
        assert_eq!(e.to_string(), "forced-av1-no-client");
    }

    #[cfg(feature = "av1")]
    #[test]
    fn auto_with_av1_capable_client_picks_av1() {
        // AV1 sits at the top of the auto chain — must beat VP9, H.264.
        let d = choose(OperatorPref::Auto, &caps(&["av1", "vp9", "h264-baseline-3.1"], true))
            .expect("auto never errors");
        assert_eq!(d.pipeline, Pipeline::Av1);
        assert_eq!(d.reason, "auto-av1");
    }

    #[cfg(feature = "av1")]
    #[test]
    fn parse_force_pipeline_av1_returns_some_when_feature() {
        assert_eq!(parse_force_pipeline("av1"), Some(OperatorPref::Av1));
    }

    #[cfg(not(feature = "av1"))]
    #[test]
    fn parse_force_pipeline_av1_returns_none_when_no_feature() {
        assert_eq!(parse_force_pipeline("av1"), None);
    }

    #[cfg(feature = "av1")]
    #[test]
    fn wire_name_av1() {
        assert_eq!(Pipeline::Av1.wire_name(), "av1");
    }

    #[test]
    fn supports_av1_case_insensitive() {
        let mk = |codec: &str| ClientCapabilities {
            codecs: vec![codec.into()],
            ..Default::default()
        };
        assert!(mk("av1").supports_av1(), "av1 must match");
        assert!(mk("AV1").supports_av1(), "AV1 must match case-insensitively");
        assert!(mk("av01.0.08M.08").supports_av1(), "av01.* MIME family must match");
        assert!(!mk("vp9").supports_av1(), "vp9 must not match av1 check");
    }

    // Priority-chain integration — needs both vp9 and av1 features.

    #[cfg(all(feature = "av1", feature = "vp9"))]
    #[test]
    fn auto_av1_beats_vp9_when_both_advertised() {
        let d = choose(OperatorPref::Auto, &caps(&["av1", "vp9"], false))
            .expect("auto never errors");
        assert_eq!(d.pipeline, Pipeline::Av1);
        assert_eq!(d.reason, "auto-av1");
    }
}
