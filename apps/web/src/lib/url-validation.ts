// URL allowlist validation for auto-resolved tunnel URLs.
//
// The discovery worker is an unauthenticated shared service — a compromised
// worker could return an attacker-controlled URL. Without validation, the SPA
// would pin that URL and send Bearer tokens there. This module enforces that
// any auto-resolved URL must be a Cloudflare Quick Tunnel subdomain or a
// user-configured named-tunnel hostname before it can be pinned.

import { loadNamedTunnelHostname } from './api-client'

/**
 * True when `url` is a safe tunnel target:
 *   - must use https (reject http, ws, etc.)
 *   - must end in `.trycloudflare.com` (Cloudflare Quick Tunnel), OR
 *   - must exactly match or be a subdomain of an entry in `namedAllowlist`.
 *
 * Returns false for malformed URLs (try/catch around URL parse).
 */
export function isAllowedTunnelHost(url: string, namedAllowlist: string[]): boolean {
  try {
    const u = new URL(url)
    if (u.protocol !== 'https:') return false
    const host = u.hostname.toLowerCase()
    if (host.endsWith('.trycloudflare.com')) return true
    return namedAllowlist.some((d) => {
      const domain = d.toLowerCase()
      return host === domain || host.endsWith(`.${domain}`)
    })
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
