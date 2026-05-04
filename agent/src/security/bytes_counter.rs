// HTTP middleware: count response body bytes for tunnel-origin requests.
//
// Sits after `tunnel_guard` in the middleware chain. Only increments the
// `bytes_proxied` counter when:
//   1. The request carries `cf-connecting-ip` (tunnel-origin).
//   2. The response status is 2xx (successful responses only).
//
// Uses `Relaxed` ordering — the counter is an operator dashboard metric, not
// a security primitive. A few bytes lost to a race window is acceptable.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http_body_util::BodyExt;

use crate::telemetry::TelemetryState;

use super::route_scope::is_tunnel_request;

pub async fn bytes_counter(
    State(telemetry): State<Arc<TelemetryState>>,
    req: Request,
    next: Next,
) -> Response {
    let is_tunnel = is_tunnel_request(req.headers());

    let response = next.run(req).await;

    if !is_tunnel || !response.status().is_success() {
        return response;
    }

    // Collect the body to measure its size, then re-stream it. For most
    // operator-dashboard 2xx responses (JSON, small frames) this is a
    // zero-allocation clone of bytes already buffered. For large file
    // downloads it buffers the body — acceptable because those are local
    // workspace operations and throughput is loop-back-bounded anyway.
    let (parts, body) = response.into_parts();
    match body.collect().await {
        Ok(collected) => {
            let bytes = collected.to_bytes();
            telemetry.add_bytes(bytes.len() as u64);
            Response::from_parts(parts, Body::from(bytes))
        }
        Err(_) => {
            // Body collection failed (rare: client disconnect mid-stream).
            // Reconstruct a minimal 200 body so the response round-trips.
            Response::from_parts(parts, Body::empty())
        }
    }
}
