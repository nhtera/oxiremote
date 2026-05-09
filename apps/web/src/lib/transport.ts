/**
 * LAN-first transport selection.
 *
 * On first use, race a HEAD probe between:
 *   - `window.location.origin` (default; the tunnel URL when accessed remotely)
 *   - `localStorage['oxi_lan_base']` (optional LAN URL configured by the user)
 *
 * Pick LAN only if it responds AND is at least `LAN_WIN_MARGIN_MS` faster than
 * the tunnel. Otherwise stick with the tunnel. Cache the winner in sessionStorage
 * so subsequent calls in the same tab skip the race.
 *
 * mDNS discovery is out of scope — user configures the LAN base explicitly.
 */

const CACHE_KEY = 'oxi_api_base'
const CACHE_RTT_KEY = 'oxi_api_base_rtt'
const LAN_STORAGE_KEY = 'oxi_lan_base'
const PROBE_TIMEOUT_MS = 200
const LAN_WIN_MARGIN_MS = 50
const PROBE_PATH = '/api/host'

let inflight: Promise<string> | null = null

export type TransportKind = 'lan' | 'tunnel' | 'unknown'

export interface TransportInfo {
  base: string
  transport: TransportKind
  latencyMs: number | null
}

function readCachedRtt(): number | null {
  try {
    const raw = sessionStorage.getItem(CACHE_RTT_KEY)
    if (!raw) return null
    const n = parseFloat(raw)
    return Number.isFinite(n) ? Math.round(n) : null
  } catch {
    return null
  }
}

function persistRtt(ms: number | null) {
  try {
    if (ms == null) sessionStorage.removeItem(CACHE_RTT_KEY)
    else sessionStorage.setItem(CACHE_RTT_KEY, String(Math.round(ms)))
  } catch {
    /* ignore */
  }
}

function readLanBase(): string | null {
  try {
    const raw = localStorage.getItem(LAN_STORAGE_KEY)?.trim()
    if (!raw) return null
    // strip trailing slash so join is predictable
    return raw.replace(/\/+$/, '')
  } catch {
    return null
  }
}

function cached(): string | null {
  try {
    return sessionStorage.getItem(CACHE_KEY)
  } catch {
    return null
  }
}

function persist(base: string) {
  try {
    sessionStorage.setItem(CACHE_KEY, base)
  } catch {
    /* ignore quota / private mode */
  }
}

async function probe(base: string, timeoutMs: number): Promise<number | null> {
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), timeoutMs)
  const started = performance.now()
  try {
    const res = await fetch(base + PROBE_PATH, {
      method: 'HEAD',
      credentials: 'include',
      signal: ctrl.signal,
    })
    // any response (even 401) proves reachability at this layer
    if (res.status >= 200 && res.status < 500) {
      return performance.now() - started
    }
    return null
  } catch {
    return null
  } finally {
    clearTimeout(timer)
  }
}

async function race(tunnel: string, lan: string): Promise<{ winner: string; rtt: number | null }> {
  const [lanRtt, tunnelRtt] = await Promise.all([
    probe(lan, PROBE_TIMEOUT_MS),
    probe(tunnel, PROBE_TIMEOUT_MS),
  ])
  if (lanRtt !== null && (tunnelRtt === null || lanRtt + LAN_WIN_MARGIN_MS <= tunnelRtt)) {
    return { winner: lan, rtt: lanRtt }
  }
  return { winner: tunnel, rtt: tunnelRtt }
}

export async function getApiBase(): Promise<string> {
  const c = cached()
  if (c) return c

  const tunnel = window.location.origin
  const lan = readLanBase()
  if (!lan || lan === tunnel) {
    persist(tunnel)
    // Tunnel-only mode: skip the probe; latency is unknown until something
    // actually fires. Leave cache empty so the pill renders without a number.
    return tunnel
  }

  if (!inflight) {
    inflight = race(tunnel, lan).then(({ winner, rtt }) => {
      persist(winner)
      persistRtt(rtt)
      inflight = null
      return winner
    })
  }
  return inflight
}

export function resetApiBase() {
  try {
    sessionStorage.removeItem(CACHE_KEY)
    sessionStorage.removeItem(CACHE_RTT_KEY)
  } catch {
    /* ignore */
  }
}

/** Synchronous read of the cached transport info — returns 'unknown' until
 *  the race resolves (callers can `await ensureTransport()` to force it). */
export function readTransportInfo(): TransportInfo {
  const tunnel = (typeof window !== 'undefined' ? window.location.origin : '') || ''
  const base = cached()
  if (!base) {
    return { base: tunnel, transport: 'unknown', latencyMs: null }
  }
  return {
    base,
    transport: base === tunnel ? 'tunnel' : 'lan',
    latencyMs: readCachedRtt(),
  }
}

/** Force resolve transport (kicks the race if still pending). */
export async function ensureTransport(): Promise<TransportInfo> {
  await getApiBase()
  return readTransportInfo()
}

// ─── Stale-URL fallback ──────────────────────────────────────────────────────

/** Thrown when the host is unreachable and the discovery fallback is not
 *  configured or has no mapping. Callers can `instanceof` check this to show
 *  a "host offline" UI instead of a generic network error. */
export class HostUnreachableError extends Error {
  cachedUrl: string
  resolvedUrl: string | null
  constructor(message: string, cachedUrl: string, resolvedUrl: string | null = null) {
    super(message)
    this.name = 'HostUnreachableError'
    this.cachedUrl = cachedUrl
    this.resolvedUrl = resolvedUrl
  }
}

/** Thrown when the discovery worker returns a URL that does not match the
 *  `*.trycloudflare.com` allowlist or the operator's named-tunnel domain.
 *  The original cached URL is NOT replaced. */
export class SuspiciousTunnelUrlError extends Error {
  suspiciousUrl: string
  constructor(suspiciousUrl: string) {
    super(
      `Discovery worker returned a URL outside the allowed domain list: ${suspiciousUrl}. ` +
        'The cached URL has NOT been updated.',
    )
    this.name = 'SuspiciousTunnelUrlError'
    this.suspiciousUrl = suspiciousUrl
  }
}

/** Symbol used to mark a request as already-retried so the fallback path
 *  does not loop. Attached as a property on the `init` object. */
const RETRY_MARKER = Symbol.for('oxi.host-fetch.retried')

/** Emitted on the window when the fallback path successfully re-resolves
 *  the tunnel URL and retries the original request. A React component
 *  (host-moved-toast) listens to this and surfaces a brief toast. */
export const HOST_MOVED_EVENT = 'oxi:host-moved'

/** Classify whether a fetch error or response looks like the host is
 *  unreachable (vs. a normal API error like 401 / 422). */
function isHostUnreachable(err: unknown, res: Response | null): boolean {
  // TypeError: network error, CORS failure, DNS resolution, TLS failure.
  if (err instanceof TypeError) return true
  // A 5xx response from Cloudflare when the origin is down (530 = DNS error,
  // 502 / 503 / 504 = origin timeout). Do NOT treat 4xx as unreachable — the
  // agent is alive, the request just failed (auth, validation, etc.).
  if (res && res.status >= 500) return true
  return false
}

/** Convenience: relative path + credentialed fetch against current base.
 *
 * On first host-unreachable failure the fallback path fires once:
 *   1. Query the discovery worker for the current tunnel URL.
 *   2. Validate it against `*.trycloudflare.com` + named-tunnel allowlist.
 *   3. If valid and different: pin the new base, retry the request once.
 *   4. On success: emit `oxi:host-moved` so the toast component can surface
 *      "Host moved — reconnected".
 * The marker `RETRY_MARKER` prevents infinite loops.
 */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const base = await getApiBase()
  const url = path.startsWith('http') ? path : base + path

  // Cast to access the retry marker property.
  const opts = init as RequestInit & { [key: symbol]: boolean }
  const alreadyRetried = opts[RETRY_MARKER] === true

  let res: Response | null = null
  let fetchErr: unknown = null
  try {
    res = await fetch(url, { credentials: 'include', ...init })
    if (!isHostUnreachable(null, res)) return res
    // 5xx from CF/origin — treat as unreachable, fall through to retry.
  } catch (err) {
    if (!isHostUnreachable(err, null)) throw err
    fetchErr = err
  }

  // Already retried on this request, or discovery isn't configured → rethrow.
  if (alreadyRetried) {
    if (fetchErr) throw fetchErr
    return res!
  }

  // Dynamic imports avoid circular dependency:
  // api-client.ts → transport.ts (static), transport.ts → api-client.ts (dynamic).
  const [{ getCurrentTunnelUrl }, { getActiveHost, loadTunnelBase, storeTunnelBase }, { isAllowedTunnelHost, getNamedTunnelAllowlist }] =
    await Promise.all([
      import('./discovery-client'),
      import('./api-client'),
      import('./url-validation'),
    ])

  const cachedUrl = loadTunnelBase() ?? base
  let newUrl: string | null = null
  try {
    newUrl = await getCurrentTunnelUrl()
  } catch {
    // Worker unreachable — fall through to re-throw original.
  }

  if (!newUrl || newUrl === cachedUrl) {
    if (fetchErr) throw new HostUnreachableError(`Host unreachable: ${cachedUrl}`, cachedUrl, null)
    return res!
  }

  const allowlist = getNamedTunnelAllowlist()
  if (!isAllowedTunnelHost(newUrl, allowlist)) {
    throw new SuspiciousTunnelUrlError(newUrl)
  }

  // Persist new base and update the session-storage transport cache so
  // subsequent same-tab fetches also use the new URL without a race.
  const activeHostId = getActiveHost()
  if (activeHostId) storeTunnelBase(activeHostId, newUrl)
  // Also update session-storage base cache used by `getApiBase`.
  try { sessionStorage.setItem(CACHE_KEY, newUrl) } catch { /* ignore */ }

  // Retry with the rebased URL, marked to prevent a second retry.
  const newFetchUrl = path.startsWith('http')
    ? path.replace(/^https?:\/\/[^/]+/, newUrl)
    : newUrl + path
  const retryInit: typeof opts = { ...init, [RETRY_MARKER]: true }
  const retryRes = await fetch(newFetchUrl, { credentials: 'include', ...retryInit })

  // Emit success event so the toast component can surface recovery feedback.
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(HOST_MOVED_EVENT))
  }

  return retryRes
}
