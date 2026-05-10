// Reverse-proxy handler for /proxy/<discovery_id>/<upstream_path>.
//
// Why this exists: the SPA's browser resolves `*.trycloudflare.com` via the
// user's local OS resolver, which can lag behind Cloudflare's authoritative
// DNS for tens of seconds after a fresh Quick Tunnel spawn or a
// sleep/wake-induced rotation. The Worker's `fetch()` runs on Cloudflare's
// edge and resolves via Cloudflare's own resolver — independent of the
// SPA's local resolver. Routing the SPA's pair traffic through the Worker
// makes DNS lag invisible.
//
// Trust boundaries:
//   - The proxy is an open relay to any agent registered with the discovery
//     worker. Agent-side auth (Bearer + CSRF + tunnel_guard) remains the
//     only trust boundary. The proxy MUST NOT carry implicit trust.
//   - We strip identifying headers (`cf-*`, `host`, `origin`, `referer`) so
//     the upstream agent doesn't trust them. The agent's `tunnel_guard`
//     keys off `cf-connecting-ip` injected by cloudflared at the upstream
//     side; that header is added between the worker and the agent (not by
//     the SPA), so stripping the SPA's view of `cf-*` is safe.
//   - CORS is owned by this proxy on the SPA-facing side. Upstream ACAO
//     headers are dropped and replaced.
//
// Wire format:
//   - HTTP: standard fetch pass-through. Body streamed via `duplex: 'half'`
//     for non-GET/HEAD methods (file uploads).
//   - WebSocket: detected via `Upgrade: websocket` request header. Workers
//     handle WS pass-through natively via `fetch()` — we return the
//     upstream Response directly (no body re-wrap; the body IS the WS
//     frame stream).

import { resolveProxyTarget, type KVLike } from './kv-store'

export interface ProxyEnv {
  DISCOVERY: KVLike
}

const HEX64 = /^[a-f0-9]{64}$/
// Cloudflare Worker subrequest cap is 30 s; leave 5 s headroom so the worker
// can send a clean 504 instead of a runtime kill.
const UPSTREAM_TIMEOUT_MS = 25_000
// Hop-by-hop / identifying headers we strip before forwarding upstream. The
// upstream agent reconstructs the auth context from `Authorization` +
// `X-OXI-CSRF` (which we pass through), not from `Origin` / `Host`.
const STRIPPED_REQUEST_HEADERS = new Set<string>([
  'host',
  'origin',
  'referer',
  'x-real-ip',
  'x-forwarded-for',
  'x-forwarded-proto',
  'x-forwarded-host',
])

/**
 * Decide whether the incoming request asks for a WebSocket upgrade. Workers'
 * `fetch()` natively pass-throughs the upgrade when both the request and
 * upstream agree on it; we only need this flag to skip body re-wrap on the
 * response (the body IS the live WS frame stream).
 */
export function isWebSocketUpgrade(req: Request): boolean {
  return req.headers.get('upgrade')?.toLowerCase() === 'websocket'
}

/**
 * Strict CORS for the proxy. Differs from the JSON-API `corsHeaders`:
 *   - No origin echoed unless explicitly allowed (caller is responsible for
 *     gating on `isAllowedOrigin` before reaching here).
 *   - Allows the headers the SPA actually uses for authed calls
 *     (`Authorization`, `X-OXI-CSRF`, `Content-Type`).
 *   - `Access-Control-Allow-Credentials: true` so the browser will send
 *     and accept the agent's CSRF cookie (set by the agent on the worker
 *     domain after pairing).
 *   - Exposes a curated subset of response headers the SPA reads
 *     (`Set-Cookie` is browser-managed, not an exposed header).
 */
export function proxyCorsHeaders(origin: string): Record<string, string> {
  return {
    'Access-Control-Allow-Origin': origin,
    'Access-Control-Allow-Credentials': 'true',
    'Access-Control-Allow-Methods': 'GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS',
    'Access-Control-Allow-Headers':
      'Authorization, Content-Type, X-OXI-CSRF, Cache-Control, Pragma',
    'Access-Control-Expose-Headers': 'Content-Type, X-OXI-CSRF',
    'Access-Control-Max-Age': '86400',
    Vary: 'Origin',
  }
}

function jsonError(message: string, status: number, origin: string | null): Response {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  if (origin) Object.assign(headers, proxyCorsHeaders(origin))
  return new Response(JSON.stringify({ error: message }), { status, headers })
}

function buildUpstreamHeaders(req: Request, workerHost: string): Headers {
  const headers = new Headers()
  for (const [name, value] of req.headers) {
    if (STRIPPED_REQUEST_HEADERS.has(name.toLowerCase())) continue
    if (name.toLowerCase().startsWith('cf-')) continue
    headers.set(name, value)
  }
  // Tell the upstream which proxy hostname brokered this request — useful
  // for log triage and any future logic that wants to distinguish proxied
  // from direct-tunnel traffic.
  headers.set('X-Forwarded-Host', workerHost)
  return headers
}

/**
 * Main entry point. `hostKey` is the discovery_id (64 hex). `upstreamPath`
 * is the unmodified suffix after `/proxy/<hostKey>` including the leading
 * `/` and any query string the caller built into it.
 *
 * Returns:
 *   - 400 if `hostKey` is malformed
 *   - 404 if the discovery_id has no registered tunnel
 *   - 502 if the upstream `fetch()` throws (network, TLS handshake failure)
 *   - 504 if the upstream takes longer than {@link UPSTREAM_TIMEOUT_MS}
 *   - upstream status otherwise (pass-through)
 *
 * `origin` is pre-validated by the caller; the proxy does not re-check
 * `isAllowedOrigin`. We require it to be present so we can emit ACAO.
 */
export async function handleProxy(
  req: Request,
  env: ProxyEnv,
  hostKey: string,
  upstreamPath: string,
  origin: string,
): Promise<Response> {
  if (!HEX64.test(hostKey)) {
    return jsonError('host_invalid', 400, origin)
  }

  const target = await resolveProxyTarget(env.DISCOVERY, hostKey)
  if (!target || !target.tunnelUrl) {
    return jsonError('host_unknown', 404, origin)
  }

  // Build the upstream URL by string concatenation. `upstreamPath` already
  // includes the query string and starts with `/`. Path traversal is not a
  // concern: the upstream agent runs its own routing — there's no shared
  // filesystem and the proxy never resolves `..`.
  const tunnelUrl = target.tunnelUrl.replace(/\/+$/, '')
  const upstreamUrl = `${tunnelUrl}${upstreamPath}`

  const workerHost = new URL(req.url).host
  const upstreamHeaders = buildUpstreamHeaders(req, workerHost)

  const isWs = isWebSocketUpgrade(req)
  const hasBody = req.method !== 'GET' && req.method !== 'HEAD'

  const init: RequestInit = {
    method: req.method,
    headers: upstreamHeaders,
    redirect: 'manual',
  }
  if (hasBody) {
    init.body = req.body
    // `duplex: 'half'` is required when the body is a streaming Request body
    // (e.g. file uploads). The fetch spec requires it but lib.dom.d.ts is
    // missing it; widen the type for the assignment.
    ;(init as RequestInit & { duplex?: string }).duplex = 'half'
  }

  // Timeout: AbortSignal.timeout returns a signal that aborts after the
  // specified delay. Workers honor abort across `fetch()`.
  const signal = AbortSignal.timeout(UPSTREAM_TIMEOUT_MS)
  init.signal = signal

  let upstreamRes: Response
  try {
    upstreamRes = await fetch(upstreamUrl, init)
  } catch (err) {
    const isAbort = err instanceof DOMException && err.name === 'TimeoutError'
    if (isAbort || (err instanceof Error && /timeout/i.test(err.message))) {
      return jsonError('upstream_timeout', 504, origin)
    }
    return jsonError('upstream_unreachable', 502, origin)
  }

  // WebSocket upgrade: return upstream directly. The body is the live frame
  // stream; re-wrapping a new Response would break the upgrade.
  if (isWs && upstreamRes.status === 101) {
    return upstreamRes
  }

  // HTTP: re-emit with worker-owned CORS headers, stripping any upstream
  // ACAO/ACAC the agent might have set (the agent doesn't know our origin).
  const respHeaders = new Headers(upstreamRes.headers)
  respHeaders.delete('access-control-allow-origin')
  respHeaders.delete('access-control-allow-credentials')
  respHeaders.delete('access-control-allow-methods')
  respHeaders.delete('access-control-allow-headers')
  respHeaders.delete('access-control-expose-headers')
  for (const [k, v] of Object.entries(proxyCorsHeaders(origin))) {
    respHeaders.set(k, v)
  }

  return new Response(upstreamRes.body, {
    status: upstreamRes.status,
    statusText: upstreamRes.statusText,
    headers: respHeaders,
  })
}
