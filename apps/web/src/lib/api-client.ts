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
import { isDiscoveryMode } from './discovery-client'

const API_KEY_PREFIX = 'oxi_api_key_'
const TUNNEL_BASE_PREFIX = 'oxi_tunnel_base_'
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
    } else {
      for (let i = localStorage.length - 1; i >= 0; i--) {
        const k = localStorage.key(i)
        if (k && k.startsWith(API_KEY_PREFIX)) {
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
 * Build a same-origin /api/<path> URL relative to the active tunnel base.
 * Returns null in embedded mode or when no tunnel base is known yet.
 */
function rewriteToTunnel(url: string): string | null {
  if (!isDiscoveryMode()) return null
  const base = loadTunnelBase()
  if (!base) {
    // Discovery mode is on but the SPA has no tunnel for the active host —
    // happens when the user paired with an old build (pre-tunnel-base
    // persistence) or wiped only some localStorage keys. Loud one-shot
    // warning so a stuck tab is debuggable from the console.
    if (!warnedMissingTunnelBase && typeof console !== 'undefined') {
      warnedMissingTunnelBase = true
      console.warn(
        '[oxiremote] discovery mode is active but no tunnel base is stored for the active host. ' +
          'Same-origin /api/* requests will hit Pages and fail. Re-pair from /login to refresh.',
      )
    }
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
