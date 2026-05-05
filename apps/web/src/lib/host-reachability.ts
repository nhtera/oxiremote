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
//   - 2xx-4xx response → tunnel is alive (origin is reachable from CF edge).
//   - 5xx OR network/timeout → unreachable.
// 401/403 are NOT treated as unreachable — they mean the agent is alive
// but the key is invalid; the saved-hosts row will surface that on click
// via switchActiveHost's existing `session-expired` branch.

import { loadTunnelBase } from './api-client'
import { isDiscoveryMode } from './discovery-client'

export type HostReachability = 'unknown' | 'probing' | 'alive' | 'unreachable'

const PROBE_TIMEOUT_MS = 3000
const HEALTH_PATH = '/api/health'

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

export async function probeHost(hostId: string): Promise<HostReachability> {
  const base = tunnelBaseFor(hostId)
  if (!base) return 'unknown'
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), PROBE_TIMEOUT_MS)
  try {
    const res = await fetch(`${base}${HEALTH_PATH}`, {
      method: 'GET',
      mode: 'cors',
      cache: 'no-store',
      signal: ctrl.signal,
    })
    return res.status >= 200 && res.status < 500 ? 'alive' : 'unreachable'
  } catch {
    return 'unreachable'
  } finally {
    clearTimeout(timer)
  }
}
