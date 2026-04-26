use std::sync::Arc;

use axum::extract::{Path, State};
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
    bind_session_to_device, clear_stale_pairing_attempts, client_ip_key,
    is_valid_pairing_attempt, issue_api_key, list_trusted_devices, random_device_id, rate_limit_key,
    require_active_auth, require_auth, revoke_device, sanitize_device_label,
    should_allow_pairing_attempt, sign_session, touch_session_and_device, new_pairing_code,
    PAIRING_TTL_SECS, SESSION_TTL_SECS,
};
use crate::db::now_ts;
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


#[derive(Deserialize)]
pub struct ExchangePairingRequest {
    code: String,
    device_label: Option<String>,
}

pub async fn api_pairing_exchange(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<ExchangePairingRequest>,
) -> impl IntoResponse {
    let code = req.code.trim().to_uppercase();
    if !is_valid_pairing_attempt(&code) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    clear_stale_pairing_attempts(&state.pairing_attempts, 60);
    let ip = client_ip_key(&headers);
    let rate_key = rate_limit_key(&ip, &code);
    if !should_allow_pairing_attempt(&state.pairing_attempts, &rate_key, 5, 60) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many pairing attempts").into_response();
    }

    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let label = sanitize_device_label(req.device_label.as_deref(), user_agent);

    let res: anyhow::Result<(String, String)> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();

        let mut stmt =
            conn.prepare("SELECT expires_at, used_at FROM pairing_codes WHERE code = ?1")?;
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

        let updated = conn.execute(
            "UPDATE pairing_codes SET used_at = ?2 WHERE code = ?1 AND used_at IS NULL",
            params![code, now],
        )?;
        if updated != 1 {
            anyhow::bail!("code already used");
        }

        let session_id = Uuid::new_v4().to_string();
        let device_id = random_device_id();
        conn.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params![session_id, now, device_id],
        )?;

        Ok((session_id, device_id))
    })();

    match res {
        Ok((session_id, device_id)) => {
            // Gate on auto_approve — mirror the OTK path in api_login_one_time.
            let auto_approve = approval::get_auto_approve(&state.db_path);
            let approval_status = if auto_approve { "approved" } else { "pending" };
            let ip = client_ip_key(&headers);

            if let Err(err) = approval::insert_device_with_approval(
                &state.db_path,
                &device_id,
                &label,
                user_agent,
                &ip,
                approval_status,
            ) {
                warn!(error=%err, "device insert failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            if let Err(err) = bind_session_to_device(&state.db_path, &session_id, &device_id) {
                warn!(error=%err, "bind session to device failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            let api_key_pair = match issue_api_key(&state.db_path, &device_id) {
                Ok(pair) => pair,
                Err(err) => {
                    warn!(error=%err, "api key issuance failed");
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

            // When pending, emit DevicePending so the TUI/dashboard approval
            // queue shows the code-paired device alongside OTK-paired ones.
            if !auto_approve {
                let first_seen = crate::db::now_ts();
                state.event_bus.send(crate::events::AgentEvent::DevicePending {
                    device_id: device_id.clone(),
                    ip: ip.clone(),
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
                    "api_key": api_key_pair.0,
                    "api_key_last4": api_key_pair.1,
                    "approval_status": approval_status,
                })),
            )
                .into_response()
        }
        Err(err) => {
            warn!(error=%err, "pairing exchange failed");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

#[derive(Serialize)]
struct DeviceListResponse {
    devices: Vec<crate::auth::TrustedDevice>,
}

pub async fn api_devices_list(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let Some(_session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
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
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(session_id) = require_active_auth(&state.db_path, &state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

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

pub async fn api_me(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let Some((session_id, device_id)) =
        crate::auth::require_active_auth_with_device(&state.db_path, &state.signing_key, &jar)
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

    // Consume OTK atomically — returns error if not found, used, or expired.
    match one_time_keys::consume_otk(&state.db_path, &token) {
        Err(err) => {
            warn!(error=%err, "OTK consume failed");
            return (
                StatusCode::GONE,
                Json(serde_json::json!({"error": "token invalid, expired, or already used"})),
            )
                .into_response();
        }
        Ok(rec) => {
            let prefix: String = rec.token.chars().take(4).collect();
            state.event_bus.send(crate::events::AgentEvent::OtkUsed { token_prefix: prefix });
        }
    }

    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let ip = client_ip_key(&headers);
    let ua_str = user_agent.unwrap_or("").to_string();
    let label = sanitize_device_label(None, user_agent);

    let device_id = random_device_id();
    let auto_approve = approval::get_auto_approve(&state.db_path);
    let approval_status = if auto_approve { "approved" } else { "pending" };

    if let Err(err) = approval::insert_device_with_approval(
        &state.db_path,
        &device_id,
        &label,
        user_agent,
        &ip,
        approval_status,
    ) {
        warn!(error=%err, "OTK device insert failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Create session and bind to device.
    let session_id = uuid::Uuid::new_v4().to_string();
    let res: anyhow::Result<()> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        conn.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at, device_id) VALUES (?1, ?2, ?2, ?3)",
            params![session_id, now, device_id],
        )?;
        Ok(())
    })();
    if let Err(err) = res {
        warn!(error=%err, "OTK session insert failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if let Err(err) = bind_session_to_device(&state.db_path, &session_id, &device_id) {
        warn!(error=%err, "OTK bind session failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

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
            Json(serde_json::json!({ "status": "approved" })),
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
            "status": "pending",
        })),
    )
        .into_response()
}

// ─── Approval status poll (tunnel-accessible) ──────────────────────────────

/// GET /api/auth/approval-status — returns the approval_status for the session's device.
/// Uses require_auth (session validity only) so pending clients can still poll.
pub async fn api_auth_approval_status(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    let Some(session_id) = require_auth(&state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

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

    match result {
        Ok(status) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": status, "session_id": session_id })),
        )
            .into_response(),
        Err(err) => {
            warn!(error=%err, "approval-status lookup failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(debug_assertions)]
pub async fn login_page() -> impl IntoResponse {
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
    </style>
  </head>
  <body>
    <h1>Pair device</h1>
    <p>Enter the pairing code shown in your local agent.</p>

    <input id="code" placeholder="ABCDEFGH" maxlength="16" autocomplete="one-time-code" />
    <button id="btn">Pair</button>
    <div id="err" class="err"></div>

    <script>
      const codeEl = document.getElementById('code');
      const errEl = document.getElementById('err');
      document.getElementById('btn').addEventListener('click', async () => {
        errEl.textContent = '';
        const code = codeEl.value.trim();
        try {
          const res = await fetch('/api/pairing/exchange', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ code })
          });
          if (!res.ok) throw new Error('Invalid or expired code');
          location.href = '/';
        } catch (e) {
          errEl.textContent = e.message || 'Pairing failed';
        }
      });
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
            terminal_sessions: DashMap::<String, Arc<TerminalSession>>::new(),
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
            desktop_available: false,
            desktop_service: None,
        })
    }

    #[test]
    fn exchange_request_accepts_optional_label() {
        let req = ExchangePairingRequest {
            code: "ABC123".into(),
            device_label: Some("My iPhone".into()),
        };
        assert_eq!(req.code, "ABC123");
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
                code: pairing.code.clone(),
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
                code: pairing.code,
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
                code: "EXPIRED1".into(),
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
                code: pairing.code,
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
                code: pairing.code,
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
}

