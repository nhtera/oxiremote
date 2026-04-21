use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
#[cfg(debug_assertions)]
use axum::response::{Html, Redirect};
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
pub async fn root(jar: CookieJar) -> impl IntoResponse {
    if jar.get("oxiremote_session").is_some() {
        Redirect::to("/app").into_response()
    } else {
        Redirect::to("/login").into_response()
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
          location.href = '/app';
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
    let Some(cookie) = jar.get("oxiremote_session") else {
        return Redirect::to("/login").into_response();
    };

    if crate::auth::verify_session(&state.signing_key, cookie.value()).is_none() {
        return Redirect::to("/login").into_response();
    }

    Html(
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
    .into_response()
}
