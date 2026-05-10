// URL allowlist validation for auto-resolved tunnel URLs.
//
// The discovery worker is an unauthenticated shared service — a compromised
// worker could return an attacker-controlled URL. Without validation, the SPA
// would pin that URL and send Bearer tokens there. This module enforces that
// any auto-resolved URL must be a Cloudflare Quick Tunnel subdomain, a
// user-configured named-tunnel hostname, OR the discovery worker's own
// proxy route (Phase 2 — `<worker>/proxy/<id>`).

import { loadNamedTunnelHostname } from './api-client'
import { discoveryBaseUrl } from './discovery-client'

/**
 * True when `url` is a safe tunnel target:
 *   - must use https (reject http, ws, etc.)
 *   - must end in `.trycloudflare.com` (Cloudflare Quick Tunnel), OR
 *   - must exactly match or be a subdomain of an entry in `namedAllowlist`,
 *     OR
 *   - must be a `/proxy/<id>` path under the SPA's configured discovery
 *     worker host (the worker is itself the trust anchor for proxy URLs;
 *     accepting them keeps the stale-URL fallback path from rejecting our
 *     own Phase-2 cache writes).
 *
 * Returns false for malformed URLs (try/catch around URL parse).
 */
export function isAllowedTunnelHost(url: string, namedAllowlist: string[]): boolean {
  try {
    const u = new URL(url)
    if (u.protocol !== 'https:') return false
    const host = u.hostname.toLowerCase()
    if (host.endsWith('.trycloudflare.com')) return true
    if (
      namedAllowlist.some((d) => {
        const domain = d.toLowerCase()
        return host === domain || host.endsWith(`.${domain}`)
      })
    ) {
      return true
    }
    // Discovery worker proxy URL: hostname matches `discoveryBaseUrl()` AND
    // path starts with `/proxy/`. We require both — a bare worker hostname
    // without the proxy path is not a valid tunnel target.
    const worker = discoveryBaseUrl()
    if (worker) {
      try {
        const w = new URL(worker)
        if (host === w.hostname.toLowerCase() && u.pathname.startsWith('/proxy/')) {
          return true
        }
      } catch {
        /* invalid VITE_DISCOVERY_URL — fall through */
      }
    }
    return false
  } catch {
    return false
  }
}

/**
 * Returns the named-tunnel allowlist for the active host. Reads the cached
 * `tunnel_named_hostname` that was persisted from the last successful
 * /api/host response. Returns an empty array for Quick Tunnel users (only
 * `*.trycloudflare.com` is allowed for them).
 */
export function getNamedTunnelAllowlist(): string[] {
  const hostname = loadNamedTunnelHostname()
  return hostname ? [hostname] : []
}
