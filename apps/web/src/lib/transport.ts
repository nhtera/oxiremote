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

/** Convenience: relative path + credentialed fetch against current base. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const base = await getApiBase()
  const url = path.startsWith('http') ? path : base + path
  return fetch(url, { credentials: 'include', ...init })
}
