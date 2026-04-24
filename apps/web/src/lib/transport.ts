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
const LAN_STORAGE_KEY = 'oxi_lan_base'
const PROBE_TIMEOUT_MS = 200
const LAN_WIN_MARGIN_MS = 50
const PROBE_PATH = '/api/host'

let inflight: Promise<string> | null = null

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

async function race(tunnel: string, lan: string): Promise<string> {
  const [lanRtt, tunnelRtt] = await Promise.all([
    probe(lan, PROBE_TIMEOUT_MS),
    probe(tunnel, PROBE_TIMEOUT_MS),
  ])
  if (lanRtt !== null && (tunnelRtt === null || lanRtt + LAN_WIN_MARGIN_MS <= tunnelRtt)) {
    return lan
  }
  return tunnel
}

export async function getApiBase(): Promise<string> {
  const c = cached()
  if (c) return c

  const tunnel = window.location.origin
  const lan = readLanBase()
  if (!lan || lan === tunnel) {
    persist(tunnel)
    return tunnel
  }

  if (!inflight) {
    inflight = race(tunnel, lan).then((winner) => {
      persist(winner)
      inflight = null
      return winner
    })
  }
  return inflight
}

export function resetApiBase() {
  try {
    sessionStorage.removeItem(CACHE_KEY)
  } catch {
    /* ignore */
  }
}

/** Convenience: relative path + credentialed fetch against current base. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const base = await getApiBase()
  const url = path.startsWith('http') ? path : base + path
  return fetch(url, { credentials: 'include', ...init })
}
