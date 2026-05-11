//! Pipeline selection.
//!
//! Decides whether a desktop-streaming session runs over the legacy JPEG
//! DataChannel pipeline or the H.264 WebRTC-track pipeline. Inputs:
//!
//! 1. **Operator preference** via the `OXI_VIDEO_PIPELINE` env var:
//!    - `auto` — H.264 if feature compiled AND client supports it, else JPEG.
//!      **The new default when the env var is unset.**
//!    - `h264` — force H.264; fail-closed if the client lacks decode (no
//!      silent JPEG fallback when the operator is explicit).
//!    - `jpeg` — force JPEG.
//!    - any other value or absent → `Auto`.
//!
//! 2. **Client capability** announced via `SignalIn::CapabilitiesClient`:
//!    - `webcodecs: true` OR the `codecs` list contains a baseline-3.1 H.264
//!      entry → client can decode H.264.
//!
//! 3. **Build-time feature** `h264`: if the binary was built without it,
//!    `choose()` always resolves to `Pipeline::Jpeg` regardless of operator
//!    preference (forced H.264 returns a `forced-h264-no-feature` error).
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

/// Operator's stated preference, distinct from the final `Pipeline` because
/// `Auto` resolves at session time using client capabilities. Forced values
/// (`H264`, `Jpeg`) override client capability checks — a forced H.264 with
/// a client that can't decode is an explicit error, not a silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorPref {
    Auto,
    #[cfg(feature = "h264")]
    H264,
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

/// `forced-h264` against a client without H.264 decode. Returned as a typed
/// error so the session layer can reject the connection rather than fall
/// back silently — operator intent is "fail-closed" when the env var is
/// explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedH264Unavailable {
    /// Stable identifier used both in `tracing` logs and the optional
    /// telemetry surface. Keeping it constant lets dashboards group these.
    pub reason: &'static str,
}

impl std::fmt::Display for ForcedH264Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for ForcedH264Unavailable {}

/// Read the operator's preference at session-setup time. Never panics —
/// unknown or missing values default to `Auto`, the new safe default that
/// resolves to H.264 only when both the feature is compiled and the client
/// advertises decode support.
///
/// **Per-session override:** the WS upgrade handler may parse a
/// `?force_pipeline=jpeg|h264|auto` query param and override the result
/// returned here for the duration of one session (see `desktop_ws`).
pub fn operator_preference() -> OperatorPref {
    parse_preference(env::var(ENV_VAR).ok().as_deref())
}

/// Parse a preference string with the same semantics as `operator_preference`
/// but on an arbitrary input — used both for the env var and for the
/// per-session `?force_pipeline=` override.
///
/// Returns `None` if the value is non-empty and not one of the known
/// identifiers; callers treat that as "reject with 400" rather than silently
/// falling back, so an SPA bug doesn't get masked as a JPEG session.
pub fn parse_force_pipeline(value: &str) -> Option<OperatorPref> {
    match value {
        "auto" => Some(OperatorPref::Auto),
        #[cfg(feature = "h264")]
        "h264" => Some(OperatorPref::H264),
        // When compiled without h264, forcing "h264" via query is a client
        // error: there's no encoder to satisfy the request.
        #[cfg(not(feature = "h264"))]
        "h264" => None,
        "jpeg" => Some(OperatorPref::Jpeg),
        _ => None,
    }
}

fn parse_preference(input: Option<&str>) -> OperatorPref {
    match input {
        Some("auto") => OperatorPref::Auto,
        #[cfg(feature = "h264")]
        Some("h264") => OperatorPref::H264,
        Some("jpeg") => OperatorPref::Jpeg,
        // Empty / unknown / absent → Auto. Distinct from `parse_force_pipeline`
        // which rejects unknown values: the env var is operator-controlled
        // and we'd rather boot than refuse to serve over a typo.
        _ => OperatorPref::Auto,
    }
}

/// Describes what the client advertised in its `capabilitiesClient` message.
#[derive(Debug, Clone, Default)]
pub struct ClientCapabilities {
    pub codecs: Vec<String>,
    pub webcodecs: bool,
    /// Client opts in to a receive-side `<audio>` sink for an Opus track.
    /// Combined with the operator's `desktop_audio_enabled` setting + a
    /// successful `desktop::audio::probe_supported()` to gate the audio
    /// transceiver before `set_remote_description`. Defaults to false so
    /// older SPAs that don't send the field never get an audio m-line.
    pub audio: bool,
    /// Client believes the connection terminates on the same machine (origin
    /// is localhost / 127.* / ::1). When true the agent skips REMB-driven
    /// encoder bitrate clamping for the H.264 pipeline: Chrome's GCC
    /// collapses to ~5 kbps on loopback because it interprets CPU-contention
    /// scheduling jitter as network congestion, killing screen-share
    /// quality. Parsec and Rustdesk skip generic BWE for the same reason.
    /// Stats SSE still surfaces the raw REMB value so the overlay shows
    /// the broken estimate; only the encoder ignores it.
    pub loopback: bool,
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

/// Whether the binary was built with the `h264` feature. Centralised so
/// downstream code doesn't litter `cfg!(feature = "h264")` everywhere.
pub const H264_COMPILED: bool = cfg!(feature = "h264");

/// Decide the pipeline for this session.
///
/// Returns `Ok(Decision)` on success and `Err(ForcedH264Unavailable)` only
/// when the operator explicitly forced H.264 (env or `?force_pipeline=h264`)
/// but the client lacks decode support. The session layer turns the error
/// into a clear client-visible signal (close WS with reason) instead of
/// quietly downgrading.
pub fn choose(
    operator: OperatorPref,
    client: &ClientCapabilities,
) -> Result<Decision, ForcedH264Unavailable> {
    let _ = client; // unused on builds without `feature = "h264"`
    match operator {
        OperatorPref::Jpeg => Ok(Decision {
            pipeline: Pipeline::Jpeg,
            reason: "forced-jpeg",
        }),
        #[cfg(feature = "h264")]
        OperatorPref::H264 => {
            if client.supports_h264_baseline() {
                Ok(Decision {
                    pipeline: Pipeline::H264,
                    reason: "forced-h264",
                })
            } else {
                Err(ForcedH264Unavailable {
                    reason: "forced-h264-no-client",
                })
            }
        }
        OperatorPref::Auto => {
            #[cfg(feature = "h264")]
            {
                if client.supports_h264_baseline() {
                    return Ok(Decision {
                        pipeline: Pipeline::H264,
                        reason: "auto-h264",
                    });
                }
                Ok(Decision {
                    pipeline: Pipeline::Jpeg,
                    reason: "auto-jpeg-no-client",
                })
            }
            #[cfg(not(feature = "h264"))]
            {
                Ok(Decision {
                    pipeline: Pipeline::Jpeg,
                    reason: "auto-jpeg-no-feature",
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webcodecs_caps() -> ClientCapabilities {
        ClientCapabilities {
            webcodecs: true,
            ..Default::default()
        }
    }

    #[test]
    fn parse_force_pipeline_known_values() {
        assert_eq!(parse_force_pipeline("auto"), Some(OperatorPref::Auto));
        assert_eq!(parse_force_pipeline("jpeg"), Some(OperatorPref::Jpeg));
        #[cfg(feature = "h264")]
        assert_eq!(parse_force_pipeline("h264"), Some(OperatorPref::H264));
        // Unknown values reject — protects against typos sneaking into the
        // SPA fallback override and getting silently treated as Auto.
        assert_eq!(parse_force_pipeline("HEVC"), None);
        assert_eq!(parse_force_pipeline(""), None);
    }

    #[test]
    fn parse_preference_env_is_lenient() {
        // Unknown env values default to Auto so an operator typo doesn't
        // brick the daemon — distinct from the query-param path.
        assert_eq!(parse_preference(None), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("")), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("nope")), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("auto")), OperatorPref::Auto);
        assert_eq!(parse_preference(Some("jpeg")), OperatorPref::Jpeg);
        #[cfg(feature = "h264")]
        assert_eq!(parse_preference(Some("h264")), OperatorPref::H264);
    }

    #[test]
    fn auto_with_capable_client_picks_h264_when_compiled() {
        let caps = webcodecs_caps();
        let decision = choose(OperatorPref::Auto, &caps).expect("auto path never errors");
        #[cfg(feature = "h264")]
        {
            assert_eq!(decision.pipeline, Pipeline::H264);
            assert_eq!(decision.reason, "auto-h264");
        }
        #[cfg(not(feature = "h264"))]
        {
            assert_eq!(decision.pipeline, Pipeline::Jpeg);
            assert_eq!(decision.reason, "auto-jpeg-no-feature");
        }
    }

    #[test]
    fn auto_with_incapable_client_picks_jpeg() {
        let caps = ClientCapabilities::default();
        let decision = choose(OperatorPref::Auto, &caps).expect("auto path never errors");
        assert_eq!(decision.pipeline, Pipeline::Jpeg);
        // Reason distinguishes "client lacked support" from "binary not built
        // with h264" — useful for ops triaging high JPEG-fallback rates.
        #[cfg(feature = "h264")]
        assert_eq!(decision.reason, "auto-jpeg-no-client");
        #[cfg(not(feature = "h264"))]
        assert_eq!(decision.reason, "auto-jpeg-no-feature");
    }

    #[test]
    fn forced_jpeg_is_always_jpeg() {
        let caps = webcodecs_caps();
        let decision = choose(OperatorPref::Jpeg, &caps).unwrap();
        assert_eq!(decision.pipeline, Pipeline::Jpeg);
        assert_eq!(decision.reason, "forced-jpeg");
    }

    #[cfg(feature = "h264")]
    #[test]
    fn forced_h264_with_capable_client_picks_h264() {
        let decision = choose(OperatorPref::H264, &webcodecs_caps()).unwrap();
        assert_eq!(decision.pipeline, Pipeline::H264);
        assert_eq!(decision.reason, "forced-h264");
    }

    #[cfg(feature = "h264")]
    #[test]
    fn forced_h264_without_client_returns_error() {
        let err = choose(OperatorPref::H264, &ClientCapabilities::default())
            .expect_err("must fail-closed on forced h264 + incapable client");
        assert_eq!(err.reason, "forced-h264-no-client");
    }

    #[test]
    fn codec_list_h264_matches_case_insensitively() {
        let c = ClientCapabilities {
            codecs: vec!["H264-Baseline-3.1".to_string()],
            ..Default::default()
        };
        assert!(c.supports_h264_baseline());
    }

    #[test]
    fn wire_name_matches_protocol() {
        assert_eq!(Pipeline::Jpeg.wire_name(), "jpeg");
        #[cfg(feature = "h264")]
        assert_eq!(Pipeline::H264.wire_name(), "h264");
    }
}
