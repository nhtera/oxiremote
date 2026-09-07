// Per-host reachability probe used by the saved-hosts panel on the login page.
//
// Background: the saved-hosts list previously rendered every entry with a
// green dot whenever the browser still held an API key for that host —
// "Recently paired", not "currently reachable". Tapping a dead host opened
// the workspace page, which tried `/api/host` and the request silently
// returned Cloudflare 530 (origin DNS error / unreachable) — a useless
// experience because nothing told the user the host was down.
//
// We probe `/api/health` (unauthenticated, returns "ok") against the saved
// per-host tunnel base. The endpoint is cheap and Cloudflare returns 530 /
// 502 / 503 / 504 with CORS headers when the origin is unreachable, so the
// browser surfaces the status to JS rather than throwing a CORS error.
//
// Heuristic:
//   - 2xx-4xx response FROM THE AGENT → tunnel is alive (origin is reachable
//     from the CF edge).
//   - 5xx OR network/timeout → unreachable.
//   - a response the discovery-worker proxy generated itself (see
//     `X-OXI-Proxy-Error` below) → never counts as proof of life.
// 401/403 are NOT treated as unreachable — they mean the agent is alive
// but the key is invalid; the saved-hosts row will surface that on click
// via switchActiveHost's existing `session-expired` branch.
//
// The proxy carve-out is load-bearing. Since Phase 2 the saved tunnel base
// is usually `<worker>/proxy/<discovery_id>`, so a probe reaches the Worker
// first. When the agent has been offline past the 24 h discovery-session
// TTL the Worker answers `404 {"error":"host_unknown"}` — the Worker itself
// is healthy, the host behind it is long gone. Under the plain status rule
// that 404 read as "alive": a dead host kept a green Online dot, and
// terminal-ws-hook's reconnect ladder (which calls probeHost) kept jumping
// back to its fast phase against a host that was never coming back.

import { loadDiscoveryLookupId, loadTunnelBase, proxiedTunnelUrl, storeTunnelBase } from './api-client'
import { isDiscoveryMode, lookupAny } from './discovery-client'

export type HostReachability = 'unknown' | 'probing' | 'alive' | 'unreachable'

const PROBE_TIMEOUT_MS = 3000
const HEALTH_PATH = '/api/health'
// Set by the discovery worker on errors it generates itself rather than
// relays (apps/discovery-worker/src/proxy-handler.ts). Its value is the
// error code, also echoed in the JSON body.
const PROXY_ERROR_HEADER = 'X-OXI-Proxy-Error'
// Proxy error codes that mean "no agent is reachable behind this discovery
// id". Any other proxy error (today: `rate_limited`) says nothing either way
// about the host, so it resolves to 'unknown' instead of a red dot.
const PROXY_HOST_DOWN_CODES = new Set([
  'host_unknown',
  'host_invalid',
  'upstream_unreachable',
  'upstream_timeout',
])

function tunnelBaseFor(hostId: string): string | null {
  // Discovery mode (cross-origin SPA on Cloudflare Pages): the per-host
  // tunnel URL is the only way to reach that host. Embedded mode falls
  // back to the current origin — saved-hosts in embedded mode means
  // either the host we are served from (probe of self), or another
  // host the user paired in this browser (whose tunnel base was saved
  // alongside the API key during pairing).
  const saved = loadTunnelBase(hostId)
  if (saved) return saved.replace(/\/+$/, '')
  if (!isDiscoveryMode() && typeof window !== 'undefined') {
    return window.location.origin
  }
  return null
}

/**
 * Read the proxy's self-reported error code, or null when the response came
 * from the agent. Prefers the header; falls back to sniffing the JSON body
 * so a SPA deployed ahead of the Worker still classifies correctly (the
 * agent never emits these codes, so a body match is unambiguous).
 */
async function proxyErrorCode(res: Response): Promise<string | null> {
  const tagged = res.headers.get(PROXY_ERROR_HEADER)?.trim()
  if (tagged) return tagged
  if (res.status !== 400 && res.status !== 404) return null
  if (!/^application\/json/i.test(res.headers.get('content-type') ?? '')) return null
  try {
    const body = (await res.clone().json()) as { error?: unknown }
    return typeof body.error === 'string' && PROXY_HOST_DOWN_CODES.has(body.error)
      ? body.error
      : null
  } catch {
    return null
  }
}

async function classify(res: Response): Promise<HostReachability> {
  if (res.status >= 500) return 'unreachable'
  const proxyError = await proxyErrorCode(res)
  if (proxyError) return PROXY_HOST_DOWN_CODES.has(proxyError) ? 'unreachable' : 'unknown'
  return res.status >= 200 ? 'alive' : 'unreachable'
}

async function probeBase(base: string): Promise<HostReachability> {
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), PROBE_TIMEOUT_MS)
  try {
    const res = await fetch(`${base}${HEALTH_PATH}`, {
      method: 'GET',
      mode: 'cors',
      cache: 'no-store',
      signal: ctrl.signal,
    })
    return await classify(res)
  } catch {
    return 'unreachable'
  } finally {
    clearTimeout(timer)
  }
}

/**
 * Re-resolve the host's current tunnel URL via the discovery worker and
 * persist it. Returns the new base on success, or null when the SPA isn't
 * in discovery mode, no lookup id is stored (paired pre-0.1.28), or the
 * worker has no current mapping.
 *
 * The agent re-registers its `discovery_id → tunnelUrl` mapping on every
 * cloudflared URL rotation (see agent/src/discovery.rs), so this resolves
 * to a fresh URL after sleep/wake / network handoff. Use it before
 * probeHost when the cached URL is suspected stale.
 */
export async function refreshTunnelBaseFromDiscovery(hostId: string): Promise<string | null> {
  if (!isDiscoveryMode()) return null
  const lookupId = loadDiscoveryLookupId(hostId)
  if (!lookupId) return null
  const session = await lookupAny(lookupId)
  if (!session) return null
  // Prefer the worker proxy over the raw Quick Tunnel URL — the same choice
  // getCurrentTunnelUrl() makes. Persisting the raw URL here would silently
  // drop this host off the proxy path (and back into local-DNS lag) for the
  // rest of the session, until the next boot-time migration puts it back.
  const proxied = proxiedTunnelUrl(session.discoveryId ?? lookupId)
  const fresh = (proxied ?? session.tunnelUrl).replace(/\/+$/, '')
  storeTunnelBase(hostId, fresh)
  return fresh
}

export async function probeHost(hostId: string): Promise<HostReachability> {
  const base = tunnelBaseFor(hostId)
  if (!base) return 'unknown'
  const first = await probeBase(base)
  if (first === 'alive') return 'alive'

  // Cached base might be stale after a Quick Tunnel rotation — try refreshing
  // from the discovery worker once before declaring the host unreachable.
  // No-op (returns null) when not in discovery mode or no lookup id is
  // stored (paired pre-0.1.28), in which case the first probe stands.
  const fresh = await refreshTunnelBaseFromDiscovery(hostId)
  if (!fresh || fresh === base) return first
  return probeBase(fresh)
}
