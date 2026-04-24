// CSRF double-submit cookie.
//
// On every request, if the client doesn't already carry the `oxi_csrf` cookie,
// we set one. State-changing (non-safe) methods arriving via the tunnel must
// then echo that token back in the `X-OXI-CSRF` header — same-origin XHR from
// our own SPA can read the cookie and do this; a cross-site attacker cannot.
//
// Loopback requests are exempt: they only come from the CLI and the dev proxy.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::Rng;

use crate::AppState;

use super::route_scope::is_tunnel_request;

pub const CSRF_COOKIE: &str = "oxi_csrf";
pub const CSRF_HEADER: &str = "x-oxi-csrf";
const TOKEN_LEN: usize = 32;

fn random_token() -> String {
    let bytes: [u8; TOKEN_LEN] = rand::rng().random();
    hex::encode(bytes)
}

fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(&format!("{name}=")) {
            return Some(rest);
        }
    }
    None
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn subtle_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().len() == b.as_bytes().len() && a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub async fn csrf_guard(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let headers = req.headers();
    let tunnel = is_tunnel_request(headers);
    let existing = cookie_value(headers, CSRF_COOKIE).map(str::to_string);

    if tunnel && is_state_changing(req.method()) {
        let cookie_tok = existing.as_deref();
        let header_tok = header_value(headers, CSRF_HEADER);
        match (cookie_tok, header_tok) {
            (Some(c), Some(h)) if subtle_eq(c, h) => {}
            _ => {
                return Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .body(Body::from("csrf token mismatch"))
                    .unwrap();
            }
        }
    }

    let mut response = next.run(req).await;

    // Ensure the cookie exists on the response so the SPA can mirror it into
    // the CSRF header on the next POST.
    if existing.is_none() {
        let token = random_token();
        let cookie = Cookie::build((CSRF_COOKIE, token))
            .http_only(false) // the SPA must read it
            .secure(state.secure_cookies)
            .same_site(SameSite::Lax)
            .path("/")
            .build();
        let jar = CookieJar::new().add(cookie);
        for c in jar.iter() {
            if let Ok(value) = c.encoded().to_string().parse() {
                response.headers_mut().append(axum::http::header::SET_COOKIE, value);
            }
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_changing_detected() {
        assert!(is_state_changing(&Method::POST));
        assert!(is_state_changing(&Method::DELETE));
        assert!(is_state_changing(&Method::PATCH));
        assert!(!is_state_changing(&Method::GET));
        assert!(!is_state_changing(&Method::HEAD));
        assert!(!is_state_changing(&Method::OPTIONS));
    }

    #[test]
    fn cookie_value_extracts_named_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "foo=bar; oxi_csrf=abcdef; baz=qux".parse().unwrap(),
        );
        assert_eq!(cookie_value(&headers, "oxi_csrf"), Some("abcdef"));
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[test]
    fn subtle_eq_is_correct() {
        assert!(subtle_eq("abc", "abc"));
        assert!(!subtle_eq("abc", "abd"));
        assert!(!subtle_eq("abc", "abcd"));
    }

    #[test]
    fn random_token_has_expected_length() {
        assert_eq!(random_token().len(), TOKEN_LEN * 2);
    }
}
