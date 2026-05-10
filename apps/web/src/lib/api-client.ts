/**
 * Authenticated API client.
 *
 * Wraps `apiFetch` from transport.ts with two additions required by the
 * hardened tunnel:
 *   - `Authorization: Bearer <api_key>`  — per-host device key issued at pairing
 *   - `X-OXI-CSRF: <cookie_value>`       — double-submit CSRF token
 *
 * Also installs a global `fetch` interceptor so existing call sites that use
 * bare `fetch('/api/…')` continue to work without a per-file refactor.
 *
 * Key storage model:
 *   - Keyed per host: `oxi_api_key_<host_id>`
 *   - Persisted to localStorage on pairing response; cleared on logout.
 *   - A single "active host" pointer (`oxi_active_host`) selects which key to
 *     attach on outgoing calls — matches how multi-host routing works in UI.
 */
import { apiFetch } from './transport'
import { discoveryBaseUrl, isDiscoveryMode } from './discovery-client'

// Quick Tunnel hostname pattern — used at boot to recognize legacy
// `tunnelBase` entries that should be migrated to the proxy URL.
const TRYCLOUDFLARE_HOST_RE = /^https?:\/\/[a-z0-9-]+\.trycloudflare\.com\b/i

const API_KEY_PREFIX = 'oxi_api_key_'
const TUNNEL_BASE_PREFIX = 'oxi_tunnel_base_'
// Stable per-host worker lookup id, captured at pair time from the agent's
// pairing-exchange response. Lets a discovery-mode SPA re-resolve the
// current tunnel URL via the worker after a Quick Tunnel rotation without
// making the user re-pair.
const DISCOVERY_LOOKUP_PREFIX = 'oxi_discovery_lookup_id_'
// Named-tunnel hostname from /api/host (tunnel.toml hostname field).
// Persisted so the URL allowlist check can run synchronously during fetch
// retry without a live /api/host round-trip. Null = Quick Tunnel.
const NAMED_TUNNEL_HOSTNAME_PREFIX = 'oxi_named_tunnel_hostname_'
const ACTIVE_HOST_KEY = 'oxi_active_host'
const CSRF_COOKIE = 'oxi_csrf'

function readCookie(name: string): string | null {
  if (typeof document === 'undefined') return null
  for (const part of document.cookie.split(';')) {
    const trimmed = part.trim()
    if (trimmed.startsWith(name + '=')) {
      return trimmed.slice(name.length + 1)
    }
  }
  return null
}

export function setActiveHost(hostId: string) {
  try {
    localStorage.setItem(ACTIVE_HOST_KEY, hostId)
  } catch {
    /* quota / private */
  }
}

export function getActiveHost(): string | null {
  try {
    return localStorage.getItem(ACTIVE_HOST_KEY)
  } catch {
    return null
  }
}

export function storeApiKey(hostId: string, apiKey: string) {
  try {
    localStorage.setItem(API_KEY_PREFIX + hostId, apiKey)
    setActiveHost(hostId)
  } catch {
    /* quota */
  }
}

export function loadApiKey(hostId?: string): string | null {
  const id = hostId ?? getActiveHost()
  if (!id) return null
  try {
    return localStorage.getItem(API_KEY_PREFIX + id)
  } catch {
    return null
  }
}

export function clearApiKey(hostId?: string) {
  try {
    if (hostId) {
      localStorage.removeItem(API_KEY_PREFIX + hostId)
      localStorage.removeItem(TUNNEL_BASE_PREFIX + hostId)
      localStorage.removeItem(DISCOVERY_LOOKUP_PREFIX + hostId)
      localStorage.removeItem(NAMED_TUNNEL_HOSTNAME_PREFIX + hostId)
    } else {
      for (let i = localStorage.length - 1; i >= 0; i--) {
        const k = localStorage.key(i)
        if (
          k &&
          (k.startsWith(API_KEY_PREFIX) ||
            k.startsWith(TUNNEL_BASE_PREFIX) ||
            k.startsWith(DISCOVERY_LOOKUP_PREFIX) ||
            k.startsWith(NAMED_TUNNEL_HOSTNAME_PREFIX))
        ) {
          localStorage.removeItem(k)
        }
      }
    }
  } catch {
    /* ignore */
  }
}

/**
 * Cross-origin tunnel base storage. In discovery mode the SPA lives on
 * Pages but the agent's API lives on the per-host tunnel — the base is
 * resolved once at pairing time (via the discovery worker) and reused for
 * every subsequent /api/* fetch. Embedded mode never writes these keys.
 */
export function storeTunnelBase(hostId: string, baseUrl: string) {
  try {
    localStorage.setItem(TUNNEL_BASE_PREFIX + hostId, baseUrl.replace(/\/$/, ''))
  } catch {
    /* quota / private mode */
  }
}

export function loadTunnelBase(hostId?: string): string | null {
  const id = hostId ?? getActiveHost()
  if (!id) return null
  try {
    const v = localStorage.getItem(TUNNEL_BASE_PREFIX + id)
    return v && v.length > 0 ? v : null
  } catch {
    return null
  }
}

/**
 * Discovery worker lookup id storage. Captured from the pairing-exchange
 * response. Persisted only in discovery mode — the worker route is the only
 * thing that uses it. Old SPAs / agents pre-0.1.28 won't have this key set;
 * callers must treat null as "no refresh path available, use cached base".
 */
export function storeDiscoveryLookupId(hostId: string, lookupId: string) {
  try {
    localStorage.setItem(DISCOVERY_LOOKUP_PREFIX + hostId, lookupId)
  } catch {
    /* quota / private mode */
  }
}

export function loadDiscoveryLookupId(hostId?: string): string | null {
  const id = hostId ?? getActiveHost()
  if (!id) return null
  try {
    const v = localStorage.getItem(DISCOVERY_LOOKUP_PREFIX + id)
    return v && v.length > 0 ? v : null
  } catch {
    return null
  }
}

/**
 * Build the Worker-proxy base URL for a given host's `discovery_id`. Returns
 * null when discovery is not active (no `VITE_DISCOVERY_URL`) — caller
 * should fall back to the direct tunnel URL.
 *
 * Wire shape: `<worker>/proxy/<discovery_id>` (no trailing slash). Caller
 * appends `/api/...` or `/ws/...` directly.
 */
export function proxiedTunnelUrl(discoveryId: string, base?: string): string | null {
  const root = (base ?? discoveryBaseUrl()).replace(/\/+$/, '')
  if (!root) return null
  // Discovery_id is hex64 in production; we don't enforce here so tests can
  // exercise the helper with shorter fixtures. The agent + worker both gate
  // on shape independently.
  return `${root}/proxy/${discoveryId}`
}

/**
 * Migrate a legacy cached `tunnelBase` (pointing at a Quick Tunnel
 * `*.trycloudflare.com` host) to the Phase-2 proxy URL. Idempotent: only
 * rewrites when the input matches the legacy shape AND a proxy URL can be
 * built (discovery active + lookup id persisted). Returns the new value
 * for caller convenience, or null when no rewrite was needed.
 */
export function migrateTunnelBaseToProxy(hostId: string): string | null {
  const cached = loadTunnelBase(hostId)
  if (!cached || !TRYCLOUDFLARE_HOST_RE.test(cached)) return null
  const lookupId = loadDiscoveryLookupId(hostId)
  if (!lookupId) return null
  const proxied = proxiedTunnelUrl(lookupId)
  if (!proxied) return null
  storeTunnelBase(hostId, proxied)
  return proxied
}

/**
 * Migrate every paired host's stale Quick Tunnel base to the proxy URL.
 * Called once at SPA boot. Skipping a host (no lookup id) is silent — old
 * agents that never registered a discovery_id keep their direct base.
 */
export function migrateAllTunnelBasesToProxy(): void {
  if (typeof localStorage === 'undefined') return
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i)
      if (!k || !k.startsWith(TUNNEL_BASE_PREFIX)) continue
      const hostId = k.slice(TUNNEL_BASE_PREFIX.length)
      migrateTunnelBaseToProxy(hostId)
    }
  } catch {
    /* private mode / quota — best-effort */
  }
}

/**
 * Named-tunnel hostname storage. Populated from /api/host response
 * (`tunnel_named_hostname` field) and used by the URL allowlist to permit
 * auto-resolve to the operator's configured named-tunnel domain.
 * Stores empty string to represent explicit null (Quick Tunnel).
 */
export function storeNamedTunnelHostname(hostId: string, hostname: string | null) {
  try {
    localStorage.setItem(NAMED_TUNNEL_HOSTNAME_PREFIX + hostId, hostname ?? '')
  } catch {
    /* quota / private mode */
  }
}

export function loadNamedTunnelHostname(hostId?: string): string | null {
  const id = hostId ?? getActiveHost()
  if (!id) return null
  try {
    const v = localStorage.getItem(NAMED_TUNNEL_HOSTNAME_PREFIX + id)
    // Empty string = Quick Tunnel (explicitly null). Missing key = not yet fetched.
    if (v === null) return null
    return v.length > 0 ? v : null
  } catch {
    return null
  }
}

function isApiPath(url: string): boolean {
  if (url.startsWith('/api/') || url.startsWith('/preview/')) return true
  try {
    const u = new URL(url, window.location.origin)
    if (u.origin === window.location.origin) {
      return u.pathname.startsWith('/api/') || u.pathname.startsWith('/preview/')
    }
  } catch {
    /* not a URL */
  }
  return false
}

function withAuthHeaders(init: RequestInit = {}): RequestInit {
  const headers = new Headers(init.headers ?? {})
  if (!headers.has('Authorization')) {
    const key = loadApiKey()
    if (key) headers.set('Authorization', `Bearer ${key}`)
  }
  const method = (init.method ?? 'GET').toUpperCase()
  if (method !== 'GET' && method !== 'HEAD' && method !== 'OPTIONS') {
    const csrf = readCookie(CSRF_COOKIE)
    if (csrf && !headers.has('X-OXI-CSRF')) {
      headers.set('X-OXI-CSRF', csrf)
    }
  }
  return { credentials: 'include', ...init, headers }
}

export async function oxiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  return apiFetch(path, withAuthHeaders(init))
}

/**
 * Cross-origin Bearer-auth client. Used by the discovery flow on a Cloudflare
 * Pages SPA where the tunnel lives on a different origin (no cookies).
 * Caller is responsible for passing absolute paths starting with `/`.
 */
export type RemoteClient = {
  fetch: (path: string, init?: RequestInit) => Promise<Response>
  baseUrl: string
}

export function makeRemoteClient(baseUrl: string, apiKey: string): RemoteClient {
  const trimmed = baseUrl.replace(/\/$/, '')
  return {
    baseUrl: trimmed,
    fetch: (path: string, init: RequestInit = {}) => {
      const headers = new Headers(init.headers ?? {})
      if (!headers.has('Authorization')) {
        headers.set('Authorization', `Bearer ${apiKey}`)
      }
      if (!headers.has('Content-Type') && init.body !== undefined) {
        headers.set('Content-Type', 'application/json')
      }
      return fetch(`${trimmed}${path}`, {
        ...init,
        credentials: 'omit',
        headers,
      })
    },
  }
}

let warnedMissingTunnelBase = false

/**
 * Build an absolute URL pointing at the active host's tunnel base, when the
 * stored base differs from the current page origin. Returns null when:
 *   - no tunnel base is stored for the active host (same-origin embedded path), OR
 *   - the stored base IS the current origin (same-host SPA tab — no rewrite needed).
 *
 * The cross-origin path covers two cases with one rule:
 *   - Discovery mode (SPA on Pages, agent on a tunnel)
 *   - Embedded multi-host (SPA on host A's tunnel, switched to host B)
 */
function rewriteToTunnel(url: string): string | null {
  const base = loadTunnelBase()
  if (!base) {
    // Discovery mode without a tunnel base means the SPA has no agent to
    // route to. Surface a one-shot warning so a stuck tab is debuggable.
    // Embedded mode without a base is normal pre-pair state — silent.
    if (isDiscoveryMode() && !warnedMissingTunnelBase && typeof console !== 'undefined') {
      warnedMissingTunnelBase = true
      console.warn(
        '[oxiremote] discovery mode is active but no tunnel base is stored for the active host. ' +
          'Same-origin /api/* requests will hit Pages and fail. Re-pair from /login to refresh.',
      )
    }
    return null
  }
  if (typeof window !== 'undefined' && base === window.location.origin) {
    // Same-host SPA tab: keep the existing same-origin (cookie-bearing) path.
    return null
  }
  let pathname: string
  if (url.startsWith('/')) {
    pathname = url
  } else {
    try {
      const u = new URL(url, window.location.origin)
      if (u.origin !== window.location.origin) return null
      pathname = u.pathname + u.search + u.hash
    } catch {
      return null
    }
  }
  return base + pathname
}

/**
 * Install a `window.fetch` interceptor that attaches auth headers to
 * same-origin `/api/*` and `/preview/*` requests. Idempotent; safe to call
 * multiple times (later calls no-op).
 *
 * Discovery mode (cross-origin SPA on Pages): same-origin /api/* requests
 * are rewritten to the saved tunnel base — the agent's API never lived on
 * Pages and we want all the existing call sites to keep using bare paths.
 * No cookies cross-origin; Bearer + omit credentials.
 */
let installed = false
export function installFetchInterceptor() {
  if (installed || typeof window === 'undefined') return
  installed = true
  const original = window.fetch.bind(window)
  window.fetch = async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    if (!isApiPath(url)) return original(input, init)

    // Cross-origin discovery mode: rewrite to the per-host tunnel base
    // and force credentials:'omit' (cookies don't apply cross-origin).
    const remoteUrl = rewriteToTunnel(url)
    if (remoteUrl !== null) {
      const headers = new Headers(input instanceof Request ? input.headers : init.headers ?? {})
      const key = loadApiKey()
      if (key && !headers.has('Authorization')) {
        headers.set('Authorization', `Bearer ${key}`)
      }
      // Build a fresh init from either Request or the supplied init —
      // never carry credentials:'include' across origins, that would
      // get rejected by the browser anyway.
      const baseInit: RequestInit =
        input instanceof Request
          ? {
              method: input.method,
              body:
                input.method && input.method !== 'GET' && input.method !== 'HEAD'
                  ? await input.clone().arrayBuffer()
                  : undefined,
              cache: input.cache,
              redirect: input.redirect,
              referrer: input.referrer,
              integrity: input.integrity,
            }
          : { ...init }
      return original(remoteUrl, { ...baseInit, credentials: 'omit', headers })
    }

    // Embedded mode: original cookie-based same-origin path.
    if (input instanceof Request) {
      const headers = new Headers(input.headers)
      const key = loadApiKey()
      if (key && !headers.has('Authorization')) {
        headers.set('Authorization', `Bearer ${key}`)
      }
      if (input.method && input.method.toUpperCase() !== 'GET') {
        const csrf = readCookie(CSRF_COOKIE)
        if (csrf && !headers.has('X-OXI-CSRF')) headers.set('X-OXI-CSRF', csrf)
      }
      return original(new Request(input, { headers }), init)
    }
    return original(input, withAuthHeaders(init))
  }
}
