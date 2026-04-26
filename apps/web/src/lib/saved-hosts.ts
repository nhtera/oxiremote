// localStorage-backed list of recently-paired hosts so a returning user can
// re-enter without typing or re-scanning. Capped at MAX entries; LRU on
// `last_seen`. The full API key isn't kept here — only the last 4 chars for
// display. The actual key lives in `oxi_api_key_<host_id>` (api-client.ts).

const KEY = 'oxi:saved-hosts'
const MAX = 5

export interface SavedHost {
  host_id: string
  label: string
  /** Last 4 chars of the API key — purely for the user to differentiate keys. */
  api_key_last4: string
  /** Unix milliseconds. */
  last_seen: number
}

export function listSavedHosts(): SavedHost[] {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter(isValidSavedHost)
  } catch {
    return []
  }
}

export function recordSavedHost(h: Omit<SavedHost, 'last_seen'>): void {
  const now = Date.now()
  const existing = listSavedHosts().filter((x) => x.host_id !== h.host_id)
  const next: SavedHost[] = [{ ...h, last_seen: now }, ...existing].slice(0, MAX)
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    // Storage quota exceeded / private mode — best-effort, won't appear next visit.
  }
}

export function removeSavedHost(host_id: string): void {
  const next = listSavedHosts().filter((x) => x.host_id !== host_id)
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    // Same as above; the entry may persist but tryReconnect re-cleans it.
  }
}

function isValidSavedHost(v: unknown): v is SavedHost {
  if (typeof v !== 'object' || v === null) return false
  const o = v as Record<string, unknown>
  return (
    typeof o.host_id === 'string' &&
    typeof o.label === 'string' &&
    typeof o.api_key_last4 === 'string' &&
    typeof o.last_seen === 'number'
  )
}

export function formatRelative(ts: number): string {
  const delta = Date.now() - ts
  const mins = Math.floor(delta / 60_000)
  if (mins < 1) return 'just now'
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days}d ago`
  return new Date(ts).toLocaleDateString()
}
