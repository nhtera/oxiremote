use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use axum::routing::{get, patch};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::events::AgentEvent;
use crate::env_defaults;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    let r = Router::new()
        .route("/api/host", get(api_host_info))
        .route("/api/hosts/{id}/desktop/capabilities", get(api_desktop_capabilities))
        .route("/api/devices/{id}", patch(api_device_rename));
    // Stats SSE — produced by the ABR pipeline which is now wired into
    // H.264 + VP9 + AV1 sessions. JPEG-only builds (no video codec)
    // don't expose the route.
    #[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
    let r = r.route("/api/hosts/{id}/desktop/stats", get(api_desktop_stats));
    r
}

#[derive(Deserialize)]
struct DeviceRenameBody {
    name: Option<String>,
}

/// PATCH /api/devices/{id} — tunnel-scoped device rename.
async fn api_device_rename(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(device_id): Path<String>,
    Json(body): Json<DeviceRenameBody>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let name_ref: Option<&str> = body.name.as_deref();
    match crate::auth::rename_device(&state.db_path, &device_id, name_ref) {
        Ok(()) => {
            info!(device_id = %device_id, "device renamed via host api");
            // DeviceApproved is the closest existing event that causes the SPA
            // Devices page to refresh a device row.
            state.event_bus.send(AgentEvent::DeviceApproved { device_id });
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(err) => {
            warn!(error=%err, device_id=%device_id, "rename device via host api failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

async fn api_host_info(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // `discovery_configured` is true when the discovery worker URL is active
    // (env set non-empty or bundled default in use). False only when the user
    // explicitly cleared OXI_DISCOVERY_URL= to opt out.
    let discovery_configured = env_defaults::discovery_url().is_some();
    // `tunnel_mode`: "named" when ~/.config/oxiremote/tunnel.toml was present
    // at startup; "quick" otherwise. Named tunnels have stable hostnames and
    // do not need the discovery banner.
    let tunnel_mode = if state.is_quick_tunnel { "quick" } else { "named" };
    // `tunnel_named_hostname`: the hostname from tunnel.toml when running in
    // named-tunnel mode. The SPA's URL allowlist auto-accepts this domain so
    // named-tunnel users don't need to configure anything. Null for Quick Tunnel.
    let tunnel_named_hostname: serde_json::Value = state
        .named_tunnel_hostname
        .as_deref()
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    (StatusCode::OK, Json(json!({
        "host_id": state.host_info.host_id,
        "label": state.host_info.label,
        "platform": state.host_info.platform,
        "discovery_configured": discovery_configured,
        "tunnel_mode": tunnel_mode,
        "tunnel_named_hostname": tunnel_named_hostname,
    }))).into_response()
}

/// GET /api/hosts/{id}/desktop/capabilities
///
/// Returns desktop availability, supported quality tiers, and monitor list.
/// Response is safe to call at any time — no capture is performed.
pub async fn api_desktop_capabilities(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(host_id): Path<String>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    if crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Guard against future multi-host routing: only serve this agent's own id.
    if host_id != state.host_info.host_id {
        return StatusCode::NOT_FOUND.into_response();
    }

    #[cfg(feature = "desktop")]
    {
        use crate::pipeline_selection::{operator_preference, ClientCapabilities, OperatorPref};

        let available = state.desktop_available;
        let monitors: Vec<serde_json::Value> = if available {
            desktop::list_monitors()
                .into_iter()
                .map(|m| json!({ "id": m.id, "label": m.label, "width": m.width, "height": m.height }))
                .collect()
        } else {
            vec![]
        };

        // Best-effort hint for the SPA so it can pick the right session hook
        // before the WS upgrade. We don't know the real client capabilities
        // here (that handshake happens after upgrade), so we feed `choose()`
        // an optimistic capability set: WebCodecs+H.264 baseline. That gives
        // the SPA the "what would Auto resolve to right now" answer, which is
        // what the pill tooltip references as `chosen_default`.
        let op = operator_preference();
        // Optimistic caps advertise every codec this binary can negotiate, so
        // `choose()` never hits the forced-unavailable error path for the SPA
        // pre-handshake hint.
        let optimistic_caps = ClientCapabilities {
            codecs: {
                #[cfg_attr(
                    not(any(feature = "vp9", feature = "av1")),
                    allow(unused_mut)
                )]
                let mut v = vec!["h264-baseline-3.1".to_string()];
                #[cfg(feature = "vp9")]
                v.push("vp9".to_string());
                #[cfg(feature = "av1")]
                v.push("av1".to_string());
                v
            },
            webcodecs: true,
            ..Default::default()
        };
        let (chosen_default, default_reason) =
            match crate::pipeline_selection::choose(op, &optimistic_caps) {
                Ok(d) => (d.pipeline.wire_name(), d.reason),
                // Unreachable with the optimistic caps above. Defensive default
                // so a future refactor still surfaces something rather than 500.
                Err(_e) => ("h264", "forced-unavailable"),
            };

        // `available_pipelines` is the *transport list* the binary can speak
        // at all (build-time feature gate). `preferred_pipeline` is what the
        // operator's env var picks; `chosen_default` is what `Auto` would
        // resolve to for the SPA right now.
        let mut available_pipelines = vec!["jpeg".to_string()];
        if crate::pipeline_selection::H264_COMPILED {
            available_pipelines.push("h264".to_string());
        }
        if crate::pipeline_selection::VP9_COMPILED {
            available_pipelines.push("vp9".to_string());
        }
        if crate::pipeline_selection::AV1_COMPILED {
            available_pipelines.push("av1".to_string());
        }
        let preferred_pipeline = match op {
            OperatorPref::Jpeg => "jpeg",
            OperatorPref::Auto => "auto",
            #[cfg(feature = "h264")]
            OperatorPref::H264 => "h264",
            #[cfg(feature = "vp9")]
            OperatorPref::Vp9 => "vp9",
            #[cfg(feature = "av1")]
            OperatorPref::Av1 => "av1",
        };

        // Phase-02: `audio_supported` is the single-source-of-truth probe
        // (`desktop::audio::probe_supported()`) — true only when the build
        // can actually capture+encode system audio (currently always false:
        // every platform stub returns `Unsupported` until kill-switch
        // passes). `audio_enabled` is the persisted user toggle, surfaced
        // so the SPA renders an honest greyed-out state.
        let audio_enabled = crate::settings::get_desktop_audio_enabled(&state.db_path);
        let audio_supported = desktop::audio::probe_supported();
        let stay_awake_supported = cfg!(target_os = "macos");
        let stay_awake_enabled = crate::settings::get_desktop_stay_awake(&state.db_path);
        let body = json!({
            "available": available,
            "quality_tiers": ["low", "med", "high"],
            "monitors": monitors,
            "available_pipelines": available_pipelines,
            "preferred_pipeline": preferred_pipeline,
            "chosen_default": chosen_default,
            "default_reason": default_reason,
            "audio_supported": audio_supported,
            "audio_enabled": audio_enabled,
            "stay_awake_supported": stay_awake_supported,
            "stay_awake_enabled": stay_awake_enabled,
        });
        (StatusCode::OK, Json(body)).into_response()
    }

    #[cfg(not(feature = "desktop"))]
    {
        // Headless build: no desktop crate, no audio. Probe is always false.
        let audio_enabled = crate::settings::get_desktop_audio_enabled(&state.db_path);
        let stay_awake_supported = cfg!(target_os = "macos");
        let stay_awake_enabled = crate::settings::get_desktop_stay_awake(&state.db_path);
        let body = json!({
            "available": false,
            "quality_tiers": ["low", "med", "high"],
            "monitors": [],
            "available_pipelines": ["jpeg"],
            "preferred_pipeline": "jpeg",
            "chosen_default": "jpeg",
            "default_reason": "no-desktop-feature",
            "audio_supported": false,
            "audio_enabled": audio_enabled,
            "stay_awake_supported": stay_awake_supported,
            "stay_awake_enabled": stay_awake_enabled,
        });
        (StatusCode::OK, Json(body)).into_response()
    }
}

/// GET /api/hosts/{id}/desktop/stats
///
/// 1 Hz Server-Sent Events stream of `StatsSnapshot` JSON for the
/// authenticated device's active H.264 desktop session. Returns 404
/// when no h264 session is active for this device — no polling
/// required, the SPA can `EventSource` and trust that an open stream
/// means a live session.
///
/// Auth: same as `/desktop/capabilities` — Bearer + cookie session.
/// `host_id` must match this agent's own id.
#[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
async fn api_desktop_stats(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Path(host_id): Path<String>,
) -> axum::response::Response {
    let bearer = crate::auth::extract_bearer(&headers);
    let Some(device_id) = crate::auth::require_tunnel_auth(
        &state.db_path,
        &state.signing_key,
        &jar,
        bearer.as_deref(),
    )
    .await
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if host_id != state.host_info.host_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(svc) = state.desktop_service.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((rx, pipeline)) = svc.subscribe_abr(&device_id) else {
        // No active webrtc session for this device — JPEG sessions don't
        // produce observations yet. SPA renders an empty overlay state.
        return StatusCode::NOT_FOUND.into_response();
    };
    crate::desktop_stats_sse::make_stream(rx, pipeline).into_response()
}
