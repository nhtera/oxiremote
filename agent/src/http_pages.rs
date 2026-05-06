use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
#[cfg(debug_assertions)]
use axum::response::Html;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::DateTime;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::Duration as TimeDuration;
use tracing::warn;
use uuid::Uuid;

use crate::auth::{
    bind_session_to_device_tx, clear_stale_pairing_attempts, client_ip_key,
    is_valid_pairing_attempt, issue_api_key_tx, list_trusted_devices, platform_from_ua,
    random_device_id, rate_limit_key, require_active_auth, require_auth, revoke_device,
    sanitize_device_label, should_allow_pairing_attempt, sign_session, touch_session_and_device,
    new_pairing_code, verify_permanent_key, PAIRING_TTL_SECS, SESSION_TTL_SECS,
};
use crate::db::now_ts;
use crate::security::cors_capture::capture_pairing_origin;
use crate::{approval, one_time_keys};
use crate::AppState;

#[derive(Serialize)]
pub struct StartPairingResponse {
    pub code: String,
    pub expires_at: DateTime<chrono::Utc>,
}

pub fn create_pairing_code(state: &AppState) -> anyhow::Result<StartPairingResponse> {
    let code = new_pairing_code();
    let expires_at_ts = now_ts() + PAIRING_TTL_SECS;

    let conn = Connection::open(&state.db_path)?;
    conn.execute(
        "INSERT INTO pairing_codes(code, expires_at, used_at) VALUES (?1, ?2, NULL)",
        params![code, expires_at_ts],
    )?;

    let expires_at = DateTime::<chrono::Utc>::from_timestamp(expires_at_ts, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid expires_at timestamp"))?;

    Ok(StartPairingResponse { code, expires_at })
}


/// Request body for POST /api/pairing/exchange.
///
/// Exactly one of `code` or `permanent_key` must be present and non-empty.
/// Supplying both or neither returns 400.
///
/// Schema:
/// ```json
/// { "code": "ABCDEFGH", "device_label": "iPhone 15" }
/// // OR
/// { "permanent_key": "sk-...", "device_label": "iPhone 15" }
/// ```
#[derive(Deserialize)]
pub struct ExchangePairingRequest {
    code: Option<String>,
    permanent_key: Option<String>,
    device_label: Option<String>,
}

pub async fn api_pairing_exchange(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<ExchangePairingRequest>,
) -> impl IntoResponse {
    let code_val = req.code.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let pkey_val = req.permanent_key.as_deref().map(str::trim).filter(|s| !s.is_empty());

    // Exactly one credential must be provided.
    match (code_val, pkey_val) {
        (Some(_), Some(_)) | (None, None) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
        _ => {}
    }

    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let platform = user_agent.and_then(platform_from_ua);
    let label = sanitize_device_label(req.device_label.as_deref(), user_agent);
    let ip = client_ip_key(&headers);
    let auto_approve = approval::get_auto_approve(&state.db_path);
    let approval_status = if auto_approve { "approved" } else { "pending" };

    if let Some(pkey) = pkey_val {
        // --- Permanent-key path ---
        let pkey_owned = pkey.to_string();
        let db_path = state.db_path.clone();
        // Argon2id is CPU-intensive; run on the blocking thread pool.
        let valid = tokio::task::spawn_blocking(move || verify_permanent_key(&db_path, &pkey_owned))
            .await
            .unwrap_or(false);
        if !valid {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        let res: anyhow::Result<(String, String, String, String)> = (|| {
            let mut conn = Connection::open(&state.db_path)?;
            let tx = conn.transaction()?;
            let now = now_ts();
            let session_id = Uuid::new_v4().to_string();
            let device_id = random_device_id();
            tx.execute(
                "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
                params![session_id, now, device_id],
            )?;
            approval::insert_device_with_approval_tx(
                &tx,
                &device_id,
                &label,
                user_agent,
                &ip,
                approval_status,
                platform.as_deref(),
            )?;
            bind_session_to_device_tx(&tx, &session_id, &device_id)?;
            let (api_key, api_key_last4) = issue_api_key_tx(&tx, &device_id)?;
            tx.commit()?;
            Ok((session_id, device_id, api_key, api_key_last4))
        })();

        if res.is_ok() {
            capture_pairing_origin(&headers, &state);
        }
        return build_pairing_response(res, &state, jar, auto_approve, approval_status, &ip, user_agent);
    }

    // --- OTK / pairing-code path (unchanged) ---
    let code = code_val.unwrap().to_uppercase();
    if !is_valid_pairing_attempt(&code) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    clear_stale_pairing_attempts(&state.pairing_attempts, 60);
    let rate_key = rate_limit_key(&ip, &code);
    if !should_allow_pairing_attempt(&state.pairing_attempts, &rate_key, 5, 60) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many pairing attempts").into_response();
    }

    // Atomic pairing exchange. All five writes (consume code, insert device,
    // insert session, bind session, issue api key) live inside one txn so a
    // mid-flight failure leaves no orphan rows and the code stays unconsumed
    // for retry.
    let res: anyhow::Result<(String, String, String, String)> = (|| {
        let mut conn = Connection::open(&state.db_path)?;
        let tx = conn.transaction()?;
        let now = now_ts();

        let mut stmt =
            tx.prepare("SELECT expires_at, used_at FROM pairing_codes WHERE code = ?1")?;
        let mut rows = stmt.query(params![code])?;
        let row = rows
            .next()?
            .ok_or_else(|| anyhow::anyhow!("code not found"))?;
        let expires_at: i64 = row.get(0)?;
        let used_at: Option<i64> = row.get(1)?;

        if used_at.is_some() {
            anyhow::bail!("code already used");
        }
        if now > expires_at {
            anyhow::bail!("code expired");
        }
        drop(rows);
        drop(stmt);

        let updated = tx.execute(
            "UPDATE pairing_codes SET used_at = ?2 WHERE code = ?1 AND used_at IS NULL",
            params![code, now],
        )?;
        if updated != 1 {
            anyhow::bail!("code already used");
        }

        let session_id = Uuid::new_v4().to_string();
        let device_id = random_device_id();
        tx.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params![session_id, now, device_id],
        )?;

        approval::insert_device_with_approval_tx(
            &tx,
            &device_id,
            &label,
            user_agent,
            &ip,
            approval_status,
            platform.as_deref(),
        )?;
        bind_session_to_device_tx(&tx, &session_id, &device_id)?;
        let (api_key, api_key_last4) = issue_api_key_tx(&tx, &device_id)?;

        tx.commit()?;
        Ok((session_id, device_id, api_key, api_key_last4))
    })();

    if res.is_ok() {
        capture_pairing_origin(&headers, &state);
    }
    build_pairing_response(res, &state, jar, auto_approve, approval_status, &ip, user_agent)
}

/// Shared response builder for both the pairing-code and permanent-key paths.
fn build_pairing_response(
    res: anyhow::Result<(String, String, String, String)>,
    state: &crate::AppState,
    jar: CookieJar,
    auto_approve: bool,
    approval_status: &str,
    ip: &str,
    user_agent: Option<&str>,
) -> axum::response::Response {
    match res {
        Ok((session_id, device_id, api_key, api_key_last4)) => {
            let cookie_value = sign_session(&state.signing_key, &session_id);
            let cookie = Cookie::build(("oxiremote_session", cookie_value))
                .http_only(true)
                .secure(state.secure_cookies)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(TimeDuration::seconds(SESSION_TTL_SECS))
                .build();

            // When pending, emit DevicePending so the TUI/dashboard approval
            // queue shows the paired device.
            if !auto_approve {
                let first_seen = crate::db::now_ts();
                state.event_bus.send(crate::events::AgentEvent::DevicePending {
                    device_id: device_id.clone(),
                    ip: ip.to_string(),
                    ua_parsed: user_agent.unwrap_or("").to_string(),
                    first_seen,
                });
            }

            let http_status = if auto_approve {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };

            (
                http_status,
                jar.add(cookie),
                Json(serde_json::json!({
                    "ok": true,
                    "device_id": device_id,
                    "api_key": api_key,
                    "api_key_last4": api_key_last4,
                    "approval_status": approval_status,
                    // host_id + label + platform are included so a cross-origin
                    // SPA (Cloudflare Pages) can stash the Bearer key AND show
                    // a friendly hostname in saved-hosts / topbar without a
                    // follow-up /api/host call — that endpoint still needs
                    // cookie auth which doesn't survive cross-origin.
                    "host_id": state.host_info.host_id,
                    "label": state.host_info.label,
                    "platform": state.host_info.platform,
                })),
            )
                .into_response()
        }
        Err(err) => {
            // Code-validation failures (not found / used / expired) stay 401.
            // Insert/hash failures surface as 500 — txn rolled back, credential
            // is still usable for retry.
            let msg = err.to_string();
            let code_invalid = msg.contains("code");
            warn!(error=%err, "pairing exchange failed");
            if code_invalid {
                StatusCode::UNAUTHORIZED.into_response()
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

#[derive(Serialize)]
struct DeviceListResponse {
    devices: Vec<crate::auth::TrustedDevice>,
}

pub async fn api_devices_list(State(state): State<Arc<AppState>>, jar: CookieJar, headers: HeaderMap) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    let Some(_device_id) = crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, bearer.as_deref()).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    match list_trusted_devices(&state.db_path) {
        Ok(devices) => (StatusCode::OK, Json(DeviceListResponse { devices })).into_response(),
        Err(err) => {
            warn!(error=%err, "list devices failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn api_device_revoke(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    // session_id is used for self-revoke cookie-clear; only available on cookie path.
    let session_id_opt = if bearer.is_none() {
        require_active_auth(&state.db_path, &state.signing_key, &jar)
    } else {
        None
    };
    let authed = if let Some(ref b) = bearer {
        crate::auth::require_tunnel_auth(&state.db_path, &state.signing_key, &jar, Some(b.as_str())).await.is_some()
    } else {
        session_id_opt.is_some()
    };
    if !authed {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if let Err(err) = revoke_device(&state.db_path, &id) {
        warn!(error=%err, "revoke device failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Drop the revoked device's push subs — no point keeping them alive.
    if let Ok(conn) = Connection::open(&state.db_path) {
        let _ = conn.execute(
            "DELETE FROM push_subscriptions WHERE device_id = ?1",
            rusqlite::params![id],
        );
    }

    // Self-revoke: clear the session cookie when the caller revokes their own device.
    // Only possible on the cookie path (Bearer callers have no session cookie).
    if let Some(session_id) = session_id_opt {
        let current_device: anyhow::Result<Option<String>> = (|| {
            let conn = Connection::open(&state.db_path)?;
            conn.query_row(
                "SELECT device_id FROM sessions WHERE session_id=?1",
                params![session_id],
                |row| row.get(0),
            ).optional().map_err(Into::into)
        })();

        let should_clear_cookie = matches!(current_device, Ok(Some(device_id)) if device_id == id);
        if should_clear_cookie {
            let cookie = Cookie::build(("oxiremote_session", ""))
                .http_only(true)
                .secure(state.secure_cookies)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(TimeDuration::seconds(0))
                .build();
            return (StatusCode::OK, jar.add(cookie), Json(serde_json::json!({"ok": true}))).into_response();
        }
    }

    (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
}

pub async fn api_logout(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let cookie = Cookie::build(("oxiremote_session", ""))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(TimeDuration::seconds(0))
        .build();

    (
        StatusCode::OK,
        jar.add(cookie),
        Json(serde_json::json!({"ok": true})),
    )
}

#[derive(Serialize)]
struct MeResponse {
    session_id: String,
    device_id: String,
}

pub async fn api_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    let bearer = crate::auth::extract_bearer(&headers);
    let Some((session_id, device_id)) = crate::auth::require_owner_session_with_device_dual(
        &state.db_path,
        &state.signing_key,
        &jar,
        bearer.as_deref(),
    )
    .await
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if let Err(err) = touch_session_and_device(&state.db_path, &session_id) {
        warn!(error=%err, "failed to update last_seen_at");
    }

    (
        StatusCode::OK,
        Json(MeResponse { session_id, device_id }),
    )
        .into_response()
}

// ─── One-Time Key login (tunnel-accessible) ────────────────────────────────

#[derive(Deserialize)]
pub struct OtkLoginRequest {
    token: String,
}

/// POST /api/login/one-time — consumes an OTK, creates a session + trusted_device.
/// Returns 200 `{ status: 'approved' }` when auto_approve is enabled, or
/// 202 `{ session_id, status: 'pending' }` and emits DevicePending SSE event.
pub async fn api_login_one_time(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<OtkLoginRequest>,
) -> impl IntoResponse {
    let token = req.token.trim().to_lowercase();
    if token.len() != 16 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "invalid token"}))).into_response();
    }

    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let ip = client_ip_key(&headers);
    let ua_str = user_agent.unwrap_or("").to_string();
    let label = sanitize_device_label(None, user_agent);
    let platform = user_agent.and_then(platform_from_ua);
    let auto_approve = approval::get_auto_approve(&state.db_path);
    let approval_status = if auto_approve { "approved" } else { "pending" };

    // Atomic OTK login. OTK consume runs first inside the txn; if any later
    // write fails the txn rolls back and the OTK stays consumable.
    let session_id = uuid::Uuid::new_v4().to_string();
    let device_id = random_device_id();

    enum OtkErr {
        Consume(anyhow::Error),
        Other(anyhow::Error),
    }

    let token_for_event = token.clone();
    // OTK login also issues a per-device api_key so a cross-origin SPA can
    // Bearer-auth subsequent requests — same primitive used by pairing
    // exchange. Returning the plaintext exactly once mirrors the existing
    // exchange contract.
    let res: Result<(String, String), OtkErr> = (|| {
        let mut conn = Connection::open(&state.db_path).map_err(|e| OtkErr::Other(e.into()))?;
        let tx = conn.transaction().map_err(|e| OtkErr::Other(e.into()))?;

        one_time_keys::consume_otk_tx(&tx, &token_for_event).map_err(OtkErr::Consume)?;

        // Stamp the OTK row with the device that consumed it so Recent Key
        // Usage can render the actual phone/browser instead of "unknown".
        tx.execute(
            "UPDATE one_time_keys SET consumed_by_device = ?1 WHERE token = ?2",
            params![device_id, token_for_event],
        ).map_err(|e| OtkErr::Other(e.into()))?;

        let now = now_ts();
        tx.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params![session_id, now, device_id],
        ).map_err(|e| OtkErr::Other(e.into()))?;

        approval::insert_device_with_approval_tx(
            &tx,
            &device_id,
            &label,
            user_agent,
            &ip,
            approval_status,
            platform.as_deref(),
        ).map_err(OtkErr::Other)?;

        bind_session_to_device_tx(&tx, &session_id, &device_id).map_err(OtkErr::Other)?;
        let api_key_pair = issue_api_key_tx(&tx, &device_id).map_err(OtkErr::Other)?;

        tx.commit().map_err(|e| OtkErr::Other(e.into()))?;
        Ok(api_key_pair)
    })();

    let (api_key, api_key_last4) = match res {
        Ok(pair) => {
            let prefix: String = token.chars().take(4).collect();
            state.event_bus.send(crate::events::AgentEvent::OtkUsed { token_prefix: prefix });
            capture_pairing_origin(&headers, &state);
            pair
        }
        Err(OtkErr::Consume(err)) => {
            warn!(error=%err, "OTK consume failed");
            return (
                StatusCode::GONE,
                Json(serde_json::json!({"error": "token invalid, expired, or already used"})),
            )
                .into_response();
        }
        Err(OtkErr::Other(err)) => {
            warn!(error=%err, "OTK login failed mid-txn — rolled back; token still valid");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let cookie_value = sign_session(&state.signing_key, &session_id);
    let cookie = Cookie::build(("oxiremote_session", cookie_value))
        .http_only(true)
        .secure(state.secure_cookies)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(TimeDuration::seconds(SESSION_TTL_SECS))
        .build();

    if auto_approve {
        return (
            StatusCode::OK,
            jar.add(cookie),
            Json(serde_json::json!({
                "status": "approved",
                "device_id": device_id,
                "api_key": api_key,
                "api_key_last4": api_key_last4,
                // host_id + label + platform let cross-origin SPAs (Cloudflare
                // Pages) stash the Bearer key AND show a friendly hostname in
                // saved-hosts / topbar without a follow-up /api/host call.
                "host_id": state.host_info.host_id,
                "label": state.host_info.label,
                "platform": state.host_info.platform,
            })),
        )
            .into_response();
    }

    // Emit DevicePending so TUI takeover and dashboard modal fire.
    let first_seen = now_ts();
    state.event_bus.send(crate::events::AgentEvent::DevicePending {
        device_id: device_id.clone(),
        ip: ip.clone(),
        ua_parsed: ua_str.clone(),
        first_seen,
    });

    (
        StatusCode::ACCEPTED,
        jar.add(cookie),
        Json(serde_json::json!({
            "session_id": session_id,
            "device_id": device_id,
            "api_key": api_key,
            "api_key_last4": api_key_last4,
            "host_id": state.host_info.host_id,
            "label": state.host_info.label,
            "platform": state.host_info.platform,
            "status": "pending",
        })),
    )
        .into_response()
}

// ─── Approval status poll (tunnel-accessible) ──────────────────────────────

#[derive(Deserialize)]
pub struct ApprovalStatusQuery {
    /// Discovery-mode fallback: cross-origin SPAs cannot send the
    /// SameSite=Lax session cookie set on the tunnel domain, so the SPA
    /// passes the device_id (which it received in the OTK login response)
    /// here. Embedded mode keeps using the cookie path; this is purely
    /// additive.
    pub device_id: Option<String>,
}

/// GET /api/auth/approval-status — returns the approval_status for the device.
///
/// Auth strategy is dual-track:
///   1. Session cookie (`oxiremote_session`) → look up the device the cookie's
///      session is bound to. Embedded same-origin path.
///   2. `?device_id=<id>` → cross-origin discovery-mode fallback. Reads the
///      device row directly. The device_id is a public identifier (the SPA
///      already received it in the OTK pairing response), so disclosure is
///      bounded to the lifecycle state of a known device — no credentials,
///      no enumeration of *other* devices.
///
/// Route is in `api_key_guard::EXEMPT_PREFIXES`, so pending clients without a
/// valid API key can still poll.
pub async fn api_auth_approval_status(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(q): Query<ApprovalStatusQuery>,
) -> impl IntoResponse {
    if let Some(session_id) = require_auth(&state.signing_key, &jar) {
        let result: anyhow::Result<String> = (|| {
            let conn = Connection::open(&state.db_path)?;
            let status: String = conn.query_row(
                "SELECT COALESCE(d.approval_status, 'approved')
                 FROM sessions s
                 LEFT JOIN trusted_devices d ON d.device_id = s.device_id
                 WHERE s.session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            Ok(status)
        })();
        return match result {
            Ok(status) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": status, "session_id": session_id })),
            )
                .into_response(),
            Err(err) => {
                warn!(error=%err, "approval-status lookup (cookie) failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    if let Some(device_id) = q.device_id.as_deref().filter(|s| !s.is_empty()) {
        let result: anyhow::Result<Option<String>> = (|| {
            let conn = Connection::open(&state.db_path)?;
            let row: Option<String> = conn
                .query_row(
                    "SELECT approval_status FROM trusted_devices WHERE device_id = ?1",
                    params![device_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(row)
        })();
        return match result {
            Ok(Some(status)) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": status })),
            )
                .into_response(),
            // Unknown device_id — likely the SPA polling against the wrong
            // agent or a forgotten/revoked device. Surface as 404 so the SPA
            // can decide to bail rather than spin until timeout.
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(err) => {
                warn!(error=%err, "approval-status lookup (device_id) failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    StatusCode::UNAUTHORIZED.into_response()
}

#[cfg(debug_assertions)]
pub async fn login_page() -> impl IntoResponse {
    // Debug-build fallback that auto-handles `?k=<otk>` from the QR / share
    // link. Release builds serve the embedded React SPA from rust-embed; this
    // page only runs in `cargo run` (debug). Without this `?k=` handling,
    // scanning a QR or clicking the dashboard share link landed on a blank
    // form even though the OTK was right there in the URL.
    Html(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>OxiRemote - Pair</title>
    <style>
      body { font-family: system-ui, -apple-system, sans-serif; padding: 24px; max-width: 520px; margin: 0 auto; }
      input { font-size: 18px; padding: 12px; width: 100%; box-sizing: border-box; }
      button { font-size: 18px; padding: 12px; width: 100%; margin-top: 12px; }
      .err { color: #b00020; margin-top: 12px; }
      .ok  { color: #137333; margin-top: 12px; }
      .muted { color: #555; font-size: 14px; margin-top: 8px; }
    </style>
  </head>
  <body>
    <h1>Pair device</h1>
    <p id="lead">Enter the pairing code shown in your local agent.</p>

    <input id="code" placeholder="ABCDEFGH" maxlength="16" autocomplete="one-time-code" />
    <button id="btn">Pair</button>
    <div id="err" class="err"></div>
    <div id="status" class="muted"></div>

    <script>
      const OTK_PATTERN = /^[a-z2-7]{16}$/;        // one_time_keys.rs
      const PAIR_PATTERN = /^[A-Z0-9]{6,16}$/;     // auth::PAIRING_CODE_LEN ≥ 6

      const codeEl = document.getElementById('code');
      const btnEl  = document.getElementById('btn');
      const errEl  = document.getElementById('err');
      const statusEl = document.getElementById('status');
      const leadEl = document.getElementById('lead');

      function setStatus(text)  { statusEl.textContent = text; statusEl.className = 'muted'; }
      function setError(text)   { errEl.textContent = text; }

      async function submitOtk(token) {
        const res = await fetch('/api/login/one-time', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'include',
          body: JSON.stringify({ token }),
        });
        if (res.status === 200 || res.status === 202) {
          location.href = '/';
          return true;
        }
        return false;
      }

      async function submitPairCode(code) {
        const res = await fetch('/api/pairing/exchange', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ code }),
        });
        if (!res.ok) throw new Error('Invalid or expired code');
        location.href = '/';
      }

      btnEl.addEventListener('click', async () => {
        setError('');
        const raw = codeEl.value.trim();
        try {
          if (OTK_PATTERN.test(raw.toLowerCase())) {
            if (!(await submitOtk(raw.toLowerCase()))) throw new Error('OTK expired or already used');
            return;
          }
          await submitPairCode(raw.toUpperCase());
        } catch (e) {
          setError(e.message || 'Pairing failed');
        }
      });

      // Auto-submit when arriving via QR / dashboard share link with ?k=<token>.
      const urlKey = new URLSearchParams(location.search).get('k');
      if (urlKey) {
        const lower = urlKey.toLowerCase();
        if (OTK_PATTERN.test(lower)) {
          leadEl.textContent = 'Signing you in…';
          setStatus('Using one-time key from link');
          history.replaceState({}, '', location.pathname);
          submitOtk(lower).then((ok) => {
            if (!ok) {
              leadEl.textContent = 'That one-time key is no longer valid. Generate a new one.';
              setStatus('');
            }
          }).catch((e) => setError(e.message || 'Pairing failed'));
        } else if (PAIR_PATTERN.test(urlKey.toUpperCase())) {
          codeEl.value = urlKey.toUpperCase();
          history.replaceState({}, '', location.pathname);
        }
      }
    </script>
  </body>
</html>"#,
    )
}

#[cfg(debug_assertions)]
pub async fn app_root(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> axum::response::Response {
    let authed = require_active_auth(&state.db_path, &state.signing_key, &jar).is_some();

    if authed {
        return Html(
            r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>OxiRemote</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="http://localhost:5173/src/main.tsx"></script>
  </body>
</html>"#,
        )
        .into_response();
    }

    // Not authed — show pairing code
    let code = (|| -> anyhow::Result<Option<String>> {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        let mut stmt = conn.prepare(
            "SELECT code FROM pairing_codes WHERE used_at IS NULL AND expires_at > ?1 ORDER BY expires_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![now])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    })()
    .unwrap_or(None);

    let code_display = code.unwrap_or_else(|| "—".to_string());

    Html(format!(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>OxiRemote</title>
    <style>
      body {{ font-family: system-ui, -apple-system, sans-serif; background: #0a0e14; color: #e0e0e0; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; }}
      .card {{ text-align: center; max-width: 400px; padding: 32px; }}
      h1 {{ font-size: 22px; margin-bottom: 4px; }}
      .sub {{ color: #888; font-size: 14px; margin-bottom: 24px; }}
      .code-box {{ background: #141a22; border: 1px solid #2a3040; border-radius: 12px; padding: 24px; }}
      .code {{ font-family: monospace; font-size: 32px; font-weight: bold; letter-spacing: 0.3em; color: #60a5fa; }}
      .hint {{ color: #666; font-size: 12px; margin-top: 16px; }}
    </style>
  </head>
  <body>
    <div class="card">
      <h1>OxiRemote</h1>
      <p class="sub">Enter this code on your phone to connect.</p>
      <div class="code-box">
        <div class="code">{code_display}</div>
      </div>
      <p class="hint">Waiting for device to pair…</p>
      <script>
        setInterval(async () => {{
          try {{
            const r = await fetch('/api/me');
            if (r.ok) location.reload();
          }} catch {{}}
        }}, 3000);
      </script>
    </div>
  </body>
</html>"#
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use axum::{extract::State, http::{header::SET_COOKIE, HeaderValue}};
    use axum_extra::extract::cookie::CookieJar;
    use dashmap::DashMap;

    use super::*;
    use crate::{db::init_db, local_sites, preview::PreviewTarget, terminal_pty::TerminalSession};

    fn test_state(name: &str) -> Arc<AppState> {
        let db_path = std::env::temp_dir().join(format!(
            "oxiremote-http-{name}-{}-{}.sqlite",
            std::process::id(),
            now_ts()
        ));
        let _ = std::fs::remove_file(&db_path);
        init_db(&db_path).unwrap();

        let data_dir = db_path.parent().unwrap().join(format!("oxi-http-{name}"));
        std::fs::create_dir_all(&data_dir).unwrap();

        Arc::new(AppState {
            db_path,
            signing_key: b"01234567890123456789012345678901".to_vec(),
            secure_cookies: false,
            terminal_sessions: Arc::new(DashMap::<String, Arc<TerminalSession>>::new()),
            preview_targets: DashMap::<String, PreviewTarget>::new(),
            preview_health: DashMap::new(),
            local_sites: local_sites::new_cache(),
            proxy_allowed_ports: Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            pairing_attempts: DashMap::new(),
            workspace_root: PathBuf::from("."),
            host_info: crate::host::HostInfo {
                host_id: "test-host".into(),
                label: "test".into(),
                platform: "test".into(),
            },
            vapid_keys: Arc::new(crate::push::load_or_create_vapid(&data_dir).unwrap()),
            notify_token: "test-token".to_string(),
            http_client: reqwest::Client::new(),
            preview_client: reqwest::Client::new(),
            rate_limiter: Arc::new(crate::security::rate_limit::RateLimiter::new()),
            event_bus: crate::events::EventBus::new(),
            tunnel_url: Arc::new(std::sync::RwLock::new(None)),
            latest_tunnel_step: Arc::new(std::sync::RwLock::new(None)),
            recent_logs: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            desktop_available: false,
            desktop_service: None,
            discovery_url: None,
            web_url: None,
            discovery_temp_key: Arc::new(std::sync::RwLock::new(None)),
            tunnel_shutdown: Arc::new(tokio::sync::Notify::new()),
            telemetry: Arc::new(crate::telemetry::TelemetryState::new()),
            files_activity: Arc::new(crate::files_activity::new_map()),
            cloudflared_path: None,
            cors_origins: crate::security::cors::seed_origins(&[], &[]),
        })
    }

    #[test]
    fn exchange_request_accepts_optional_label() {
        let req = ExchangePairingRequest {
            code: Some("ABC123".into()),
            permanent_key: None,
            device_label: Some("My iPhone".into()),
        };
        assert_eq!(req.code.as_deref(), Some("ABC123"));
        assert_eq!(req.device_label.as_deref(), Some("My iPhone"));
    }

    #[tokio::test]
    async fn pairing_exchange_succeeds_once_and_rejects_reuse() {
        let state = test_state("pair-reuse");

        // Enable auto_approve so this test focuses on reuse rejection, not approval gating.
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('auto_approve', 'true')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
        drop(conn);

        let pairing = create_pairing_code(state.as_ref()).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("test-device"));

        let ok = api_pairing_exchange(
            State(state.clone()),
            headers.clone(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code.clone()),
                permanent_key: None,
                device_label: Some("My Phone".into()),
            }),
        )
        .await
        .into_response();

        assert_eq!(ok.status(), StatusCode::OK);
        assert!(ok.headers().get(SET_COOKIE).is_some());

        let reused = api_pairing_exchange(
            State(state),
            headers,
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: None,
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pairing_exchange_rejects_expired_code() {
        let state = test_state("pair-expired");
        let expired_at = now_ts() - 10;
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO pairing_codes(code, expires_at, used_at) VALUES (?1, ?2, NULL)",
            params!["EXPIRED1", expired_at],
        )
        .unwrap();

        let rejected = api_pairing_exchange(
            State(state),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some("EXPIRED1".into()),
                permanent_key: None,
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }

    // F7 regression: pairing-code exchange must respect the auto_approve setting.

    #[tokio::test]
    async fn pairing_exchange_pending_when_auto_approve_off() {
        let state = test_state("pair-pending");

        // Ensure auto_approve=false (the default, but set explicitly for clarity).
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('auto_approve', 'false')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
        drop(conn);

        let pairing = create_pairing_code(state.as_ref()).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("test-device"));

        let resp = api_pairing_exchange(
            State(state.clone()),
            headers,
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: None,
                device_label: Some("Test Phone".into()),
            }),
        )
        .await
        .into_response();

        // 202 Accepted when pending, cookie still set so the client can poll.
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert!(resp.headers().get(SET_COOKIE).is_some());

        // Read body to verify approval_status field.
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["approval_status"], "pending");
        assert!(body["api_key"].is_string());
        assert!(body["device_id"].is_string());

        // Verify the DB row has approval_status='pending'.
        let device_id = body["device_id"].as_str().unwrap().to_string();
        let conn = Connection::open(&state.db_path).unwrap();
        let status: String = conn
            .query_row(
                "SELECT COALESCE(approval_status, 'approved') FROM trusted_devices WHERE device_id=?1",
                params![device_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[tokio::test]
    async fn pairing_exchange_approved_when_auto_approve_on() {
        let state = test_state("pair-auto-approve");

        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('auto_approve', 'true')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
        drop(conn);

        let pairing = create_pairing_code(state.as_ref()).unwrap();
        let resp = api_pairing_exchange(
            State(state.clone()),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: None,
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["approval_status"], "approved");
    }

    // ── S1 atomicity regression tests ────────────────────────────────────────

    /// Drop-without-commit on a transaction holding `consume_otk_tx` must
    /// leave the OTK row consumable by a subsequent attempt. This is the
    /// invariant the atomic OTK login flow relies on: if any later step
    /// fails, the rollback returns the OTK to the user.
    #[test]
    fn otk_consume_rolled_back_when_tx_dropped() {
        let state = test_state("otk-rollback");
        let rec = one_time_keys::generate_otk(&state.db_path, None).unwrap();

        {
            let mut conn = Connection::open(&state.db_path).unwrap();
            let tx = conn.transaction().unwrap();
            one_time_keys::consume_otk_tx(&tx, &rec.token).expect("consume in txn");
            // Intentional: do NOT commit. Drop rolls back.
            drop(tx);
        }

        // OTK must still be consumable.
        let conn2 = Connection::open(&state.db_path).unwrap();
        let used_at: Option<i64> = conn2
            .query_row(
                "SELECT used_at FROM one_time_keys WHERE token=?1",
                params![rec.token],
                |r| r.get(0),
            )
            .unwrap();
        assert!(used_at.is_none(), "OTK must remain unconsumed after txn rollback");

        // And consume_otk_tx must succeed on the retry.
        let tx2 = conn2.unchecked_transaction().unwrap();
        let ok = one_time_keys::consume_otk_tx(&tx2, &rec.token);
        assert!(ok.is_ok(), "OTK retry after rollback must succeed");
    }

    /// Mirror of the OTK rollback test for the pairing-code path: if any
    /// step inside the pairing transaction fails, the code stays unconsumed
    /// and the user can retry without operator intervention.
    #[tokio::test]
    async fn pairing_code_rolled_back_when_tx_dropped() {
        let state = test_state("pair-rollback");
        let pairing = create_pairing_code(state.as_ref()).unwrap();

        {
            let mut conn = Connection::open(&state.db_path).unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE pairing_codes SET used_at = ?2 WHERE code = ?1 AND used_at IS NULL",
                params![pairing.code, now_ts()],
            )
            .unwrap();
            // Drop without commit → rolls back.
            drop(tx);
        }

        let conn2 = Connection::open(&state.db_path).unwrap();
        let used_at: Option<i64> = conn2
            .query_row(
                "SELECT used_at FROM pairing_codes WHERE code=?1",
                params![pairing.code],
                |r| r.get(0),
            )
            .unwrap();
        assert!(used_at.is_none(), "pairing code must remain unconsumed after rollback");

        // Real exchange should still succeed.
        let resp = api_pairing_exchange(
            State(state),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: None,
                device_label: None,
            }),
        )
        .await
        .into_response();
        // Either 200 (auto_approve on by some prior test? no) or 202 — both
        // are success; the assertion is that it isn't 401 (already used).
        assert!(
            resp.status().is_success(),
            "pairing exchange after rollback must succeed; got {}",
            resp.status()
        );
    }

    /// Two simultaneous `api_pairing_exchange` calls with the same code:
    /// exactly one must succeed (200 or 202); the other must be rejected.
    /// Guards against the prior bug where multi-step writes raced and could
    /// both partially succeed.
    #[tokio::test]
    async fn concurrent_pairing_exchange_serializes() {
        let state = test_state("pair-concurrent");
        let pairing = create_pairing_code(state.as_ref()).unwrap();

        let s1 = state.clone();
        let s2 = state.clone();
        let code1 = pairing.code.clone();
        let code2 = pairing.code.clone();

        let (r1, r2) = tokio::join!(
            async move {
                api_pairing_exchange(
                    State(s1),
                    HeaderMap::new(),
                    CookieJar::new(),
                    Json(ExchangePairingRequest { code: Some(code1), permanent_key: None, device_label: None }),
                )
                .await
                .into_response()
                .status()
            },
            async move {
                api_pairing_exchange(
                    State(s2),
                    HeaderMap::new(),
                    CookieJar::new(),
                    Json(ExchangePairingRequest { code: Some(code2), permanent_key: None, device_label: None }),
                )
                .await
                .into_response()
                .status()
            }
        );

        let successes = [r1, r2].iter().filter(|s| s.is_success()).count();
        assert_eq!(successes, 1, "exactly one concurrent exchange must succeed; got r1={r1}, r2={r2}");
    }

    // ── Permanent-key pairing tests ──────────────────────────────────────────

    /// Helper: mint a permanent key and store it in the given test DB.
    fn setup_permanent_key(db_path: &std::path::PathBuf) -> String {
        let (key, _last4, _created_at, _lookup_id) =
            crate::auth::rotate_permanent_key(db_path).unwrap();
        key
    }

    #[tokio::test]
    async fn pairing_exchange_with_permanent_key_succeeds() {
        let state = test_state("pkey-success");
        // Enable auto_approve for a clean 200 path.
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('auto_approve', 'true')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
        drop(conn);

        let pkey = setup_permanent_key(&state.db_path);

        let resp = api_pairing_exchange(
            State(state.clone()),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: None,
                permanent_key: Some(pkey),
                device_label: Some("Permanent Key Device".into()),
            }),
        )
        .await
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["ok"], true);
        assert!(body["api_key"].is_string());
        assert!(body["device_id"].is_string());
        assert_eq!(body["approval_status"], "approved");
    }

    #[tokio::test]
    async fn pairing_exchange_with_wrong_permanent_key_rejected() {
        let state = test_state("pkey-reject");
        // Mint a real key so there is a hash in the DB — wrong key should still fail.
        let _ = setup_permanent_key(&state.db_path);

        let resp = api_pairing_exchange(
            State(state),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: None,
                permanent_key: Some("sk-wrongkeyvalue".into()),
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pairing_exchange_with_both_code_and_permanent_key_rejected_400() {
        let state = test_state("pkey-both");
        let pairing = create_pairing_code(state.as_ref()).unwrap();

        let resp = api_pairing_exchange(
            State(state),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: Some("sk-something".into()),
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn pairing_exchange_with_permanent_key_populates_platform_from_ua() {
        let state = test_state("pkey-platform");
        let conn = Connection::open(&state.db_path).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('auto_approve', 'true')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [],
        )
        .unwrap();
        drop(conn);

        let pkey = setup_permanent_key(&state.db_path);

        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_static("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)"),
        );

        let resp = api_pairing_exchange(
            State(state.clone()),
            headers,
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: None,
                permanent_key: Some(pkey),
                device_label: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let device_id = body["device_id"].as_str().unwrap();

        let conn = Connection::open(&state.db_path).unwrap();
        let platform: Option<String> = conn
            .query_row(
                "SELECT platform FROM trusted_devices WHERE device_id=?1",
                params![device_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(platform.as_deref(), Some("ios"));
    }

    /// Discovery-mode SPAs cannot send the SameSite=Lax session cookie set
    /// on the agent's tunnel domain. The handler MUST accept `?device_id=`
    /// as a fallback so the cross-origin "Waiting for approval" page can
    /// poll the agent directly. Without this the page spins until timeout.
    #[tokio::test]
    async fn approval_status_query_param_returns_pending_then_approved() {
        let state = test_state("approval-discovery-fallback");
        let pairing = create_pairing_code(state.as_ref()).unwrap();

        // Pair without auto-approve → device row has approval_status='pending'.
        let resp = api_pairing_exchange(
            State(state.clone()),
            HeaderMap::new(),
            CookieJar::new(),
            Json(ExchangePairingRequest {
                code: Some(pairing.code),
                permanent_key: None,
                device_label: None,
            }),
        )
        .await
        .into_response();
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let device_id: String = body["device_id"].as_str().unwrap().into();

        // Cookie-less, device_id-only call mimics a cross-origin SPA poll.
        let resp = api_auth_approval_status(
            State(state.clone()),
            CookieJar::new(),
            Query(ApprovalStatusQuery { device_id: Some(device_id.clone()) }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["status"], "pending");

        // Approve the device, then re-poll — same path must observe the flip.
        approval::approve_device(&state.db_path, &device_id).unwrap();
        let resp = api_auth_approval_status(
            State(state.clone()),
            CookieJar::new(),
            Query(ApprovalStatusQuery { device_id: Some(device_id.clone()) }),
        )
        .await
        .into_response();
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["status"], "approved");
    }

    #[tokio::test]
    async fn approval_status_unknown_device_id_returns_404() {
        let state = test_state("approval-unknown-device");
        let resp = api_auth_approval_status(
            State(state),
            CookieJar::new(),
            Query(ApprovalStatusQuery { device_id: Some("ghost-device-id".into()) }),
        )
        .await
        .into_response();
        // 404 lets the SPA short-circuit instead of polling until 5-min timeout.
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn approval_status_no_cookie_no_device_returns_401() {
        let state = test_state("approval-no-auth");
        let resp = api_auth_approval_status(
            State(state),
            CookieJar::new(),
            Query(ApprovalStatusQuery { device_id: None }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

