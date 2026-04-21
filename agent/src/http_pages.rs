use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
#[cfg(debug_assertions)]
use axum::response::Html;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::DateTime;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use time::Duration as TimeDuration;
use tracing::warn;
use uuid::Uuid;

use crate::auth::{
    new_pairing_code, require_auth, sign_session, PAIRING_TTL_SECS, SESSION_TTL_SECS,
};
use crate::db::now_ts;
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

pub async fn api_pairing_start(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match create_pairing_code(&state) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => {
            warn!(error = %err, "failed to create pairing code");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn api_pairing_current(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let res: anyhow::Result<Option<StartPairingResponse>> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        let mut stmt = conn.prepare(
            "SELECT code, expires_at FROM pairing_codes WHERE used_at IS NULL AND expires_at > ?1 ORDER BY expires_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![now])?;
        match rows.next()? {
            Some(row) => {
                let code: String = row.get(0)?;
                let expires_at_ts: i64 = row.get(1)?;
                let expires_at = DateTime::<chrono::Utc>::from_timestamp(expires_at_ts, 0)
                    .ok_or_else(|| anyhow::anyhow!("invalid timestamp"))?;
                Ok(Some(StartPairingResponse { code, expires_at }))
            }
            None => Ok(None),
        }
    })();

    match res {
        Ok(Some(resp)) => (StatusCode::OK, Json(serde_json::json!(resp))).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            warn!(error=%err, "failed to get current pairing code");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct ExchangePairingRequest {
    code: String,
}

pub async fn api_pairing_exchange(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<ExchangePairingRequest>,
) -> impl IntoResponse {
    let code = req.code.trim().to_uppercase();
    if code.len() < 6 || code.len() > 16 {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let res: anyhow::Result<String> = (|| {
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
        conn.execute(
            "INSERT INTO sessions(session_id, created_at, last_seen_at) VALUES (?1, ?2, ?2)",
            params![session_id, now],
        )?;

        Ok(session_id)
    })();

    match res {
        Ok(session_id) => {
            let cookie_value = sign_session(&state.signing_key, &session_id);
            let cookie = Cookie::build(("oxiremote_session", cookie_value))
                .http_only(true)
                .secure(state.secure_cookies)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(TimeDuration::seconds(SESSION_TTL_SECS))
                .build();

            (
                StatusCode::OK,
                jar.add(cookie),
                Json(serde_json::json!({"ok": true})),
            )
                .into_response()
        }
        Err(err) => {
            warn!(error=%err, "pairing exchange failed");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
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
}

pub async fn api_me(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let Some(session_id) = require_auth(&state.signing_key, &jar) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let res: anyhow::Result<()> = (|| {
        let conn = Connection::open(&state.db_path)?;
        let now = now_ts();
        conn.execute(
            "UPDATE sessions SET last_seen_at=?2 WHERE session_id=?1",
            params![session_id, now],
        )?;
        Ok(())
    })();

    if let Err(err) = res {
        warn!(error=%err, "failed to update last_seen_at");
    }

    (StatusCode::OK, Json(MeResponse { session_id })).into_response()
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
    let authed = require_auth(&state.signing_key, &jar).is_some();

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
    </div>
    <script>
      setInterval(async () => {{
        try {{
          const r = await fetch('/api/me');
          if (r.ok) location.reload();
        }} catch {{}}
      }}, 3000);
    </script>
  </body>
</html>"#
    ))
    .into_response()
}
