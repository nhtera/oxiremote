/**
 * Discovery worker client.
 *
 * Resolves the agent's current quick-tunnel URL from a short-lived temp key
 * embedded in a QR code. Activated only when `VITE_DISCOVERY_URL` is set at
 * build time — embedded-mode SPAs (served by the agent itself) never load
 * this path.
 *
 * Worker contract:
 *   GET /api/session/lookup?k=<tempKey>
 *     200 { tunnelUrl: string, localIp: string | null }
 *     404 { error: 'not found' }   // unknown / expired
 */

const TEMP_KEY_PATTERN = /^[a-f0-9]{32}$/
// Pairing codes: 8-16 uppercase alnum (agent issues 8). Match the agent-side
// `auth::PAIRING_CODE_LEN` and the worker's `/api/code/register` shape gate.
const PAIRING_CODE_PATTERN = /^[A-Z0-9]{6,16}$/
// One-time keys: 16 chars lowercase RFC4648 base32 (a-z + 2-7) — see
// `agent/src/one_time_keys.rs`. Strict shape lets the SPA disambiguate from
// pairing codes before the worker round-trip.
const OTK_PATTERN = /^[a-z2-7]{16}$/
// Permanent dashboard keys: `sk-` followed by URL-safe base64 (no padding).
// Plaintext is never sent to the worker — we hash it client-side and look up
// the 16-hex SHA-256 prefix the agent registers on rotation.
const PERMANENT_KEY_PATTERN = /^sk-[A-Za-z0-9_-]{4,}$/

export type LookupResult = {
  tunnelUrl: string
  localIp: string | null
  /**
   * Stable per-host id the discovery worker uses as its KV session key. The
   * Phase-1 worker echoes this in `/api/session/lookup` so the SPA can
   * address the worker proxy (`/proxy/<discoveryId>/...`) on the same
   * round-trip. Older workers omit it; null = SPA must fall back to the
   * direct tunnelUrl.
   */
  discoveryId: string | null
}

/** True only when the bundle is running under `vite dev` (HMR server). Vite
 *  injects `DEV` at build time — it stays false for `vite build` artifacts
 *  regardless of where they're served from (Cloudflare Pages, the embedded
 *  agent, `vite preview` on localhost). The dev server proxies `/api/*` to
 *  the local agent at 127.0.0.1:8787, so the worker round-trip is never the
 *  right path under HMR even when `VITE_DISCOVERY_URL` is set in `.env`. */
function isViteDev(): boolean {
  return import.meta.env.DEV === true
}

/** Discovery worker base URL injected at SPA build time. Empty / unset means
 *  embedded mode — callers must check `isDiscoveryMode()` before calling
 *  `lookupSession`. Always empty under `vite dev` so HMR pairs against the
 *  local agent through the Vite proxy instead of the production worker.
 */
export function discoveryBaseUrl(): string {
  if (isViteDev()) return ''
  const raw = (import.meta.env.VITE_DISCOVERY_URL ?? '').toString().trim()
  return raw.replace(/\/$/, '')
}

export function isDiscoveryMode(): boolean {
  return discoveryBaseUrl().length > 0
}

/** Cheap shape gate so callers can branch on `?k=` form before round-tripping
 *  to the worker. Temp keys are 32 lowercase hex chars; OTKs are 16 base32. */
export function isLikelyTempKey(value: string): boolean {
  return TEMP_KEY_PATTERN.test(value)
}

/** Pairing codes are 6-16 uppercase alnum (the agent issues 8 chars). The
 *  worker accepts the same shape on `/api/code/register`; returning false
 *  here lets the SPA short-circuit before a round-trip. */
export function isLikelyPairingCode(value: string): boolean {
  return PAIRING_CODE_PATTERN.test(value)
}

/** One-time keys are 16 chars lowercase base32 (RFC 4648). The agent
 *  registers them with the worker on issuance so cross-origin manual entry
 *  can resolve `?code=<otk>` -> tunnelUrl just like pairing codes. */
export function isLikelyOtk(value: string): boolean {
  return OTK_PATTERN.test(value)
}

/** Permanent dashboard keys carry the `sk-` prefix the agent assigns at
 *  rotation. Length is unbounded above (URL-safe base64 of 32 random bytes
 *  is 43 chars without padding) — the regex only enforces the prefix and
 *  alphabet so a typo doesn't get sent to the worker as a hash input. */
export function isLikelyPermanentKey(value: string): boolean {
  return PERMANENT_KEY_PATTERN.test(value)
}

/** Derive the worker-side lookup id for a permanent key. SHA-256(plaintext)
 *  truncated to 16 hex chars — non-secret, deterministic, matches the agent
 *  `auth::permanent_key_lookup_id`. Throws when SubtleCrypto is missing
 *  (insecure context); callers should surface a "use https" error. */
export async function permanentKeyLookupId(plaintext: string): Promise<string> {
  if (typeof crypto === 'undefined' || !crypto.subtle) {
    throw new Error('SubtleCrypto unavailable — sk-… pairing requires HTTPS')
  }
  const bytes = new TextEncoder().encode(plaintext)
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  const view = new Uint8Array(digest, 0, 8)
  let hex = ''
  for (const b of view) hex += b.toString(16).padStart(2, '0')
  return hex
}

/** Convenience: hash a sk-… plaintext, then resolve via the worker. Returns
 *  null when discovery is disabled, the worker is unreachable, or the lookup
 *  id is unknown / expired. Plaintext never leaves the browser. */
export async function lookupPermanentKey(plaintext: string): Promise<LookupResult | null> {
  const lookupId = await permanentKeyLookupId(plaintext.trim())
  return rawLookup(lookupId)
}

/** Generic worker lookup — used when the caller has already shape-checked
 *  the value (e.g. the login form which accepts both OTK and pairing code).
 *  Returns null on 404 / unreachable. */
export async function lookupAny(key: string): Promise<LookupResult | null> {
  return rawLookup(key)
}

/** Resolves a temp key to the agent's tunnel URL. Returns null when the key
 *  is unknown or the worker is unreachable — callers should surface an
 *  "expired QR, regenerate from the host" error. No retry: a stale key will
 *  not become valid by waiting. */
export async function lookupSession(tempKey: string): Promise<LookupResult | null> {
  if (!isLikelyTempKey(tempKey)) return null
  return rawLookup(tempKey)
}

/** Resolves a user-typed pairing code to the agent's tunnel URL. Same wire
 *  contract as `lookupSession` (worker stores pairing codes in the same
 *  temp-key index) but with a different shape gate. */
export async function lookupPairingCode(code: string): Promise<LookupResult | null> {
  if (!isLikelyPairingCode(code)) return null
  return rawLookup(code)
}

// ─── Cold-load resolver ─────────────────────────────────────────────────────
//
// Used by the fetch retry path and WS reconnect hooks when the SPA cold-loads
// with a stale cached tunnel base (agent restarted → new Quick Tunnel URL).
// The SSE path (use-tunnel-url-sse.ts) handles mid-session URL rotation;
// this path handles the "page was already open when the URL changed" case.
//
// Module-level singleton promise: all concurrent callers (transport retry +
// up to 3 WS hooks reconnecting in parallel) share one worker round-trip.

let resolveInFlight: Promise<string | null> | null = null

/**
 * Resolve the active host's current tunnel URL via the discovery worker.
 *
 * Concurrent callers receive the same in-flight promise (coalesced) so at most
 * one worker hit occurs per reconnect storm. Returns null when:
 *   - discovery mode is not active (embedded SPA), or
 *   - no `discovery_id` is stored (paired pre-0.1.28), or
 *   - the worker has no mapping for this host (worker down / unregistered).
 *
 * Phase 2: when the SPA was configured with `VITE_DISCOVERY_URL` (i.e. the
 * worker has a `/proxy/<id>/...` route) we return the **proxy URL**, not the
 * raw tunnel URL. Routing through the proxy makes the SPA's local DNS
 * irrelevant for this host. Caller still owns the URL allowlist check.
 *
 * Does NOT persist the result — callers are responsible for validating
 * against the URL allowlist before writing to localStorage.
 */
export async function getCurrentTunnelUrl(): Promise<string | null> {
  if (resolveInFlight) return resolveInFlight
  resolveInFlight = (async () => {
    try {
      // loadDiscoveryLookupId is imported lazily to avoid a circular dep at
      // module parse time (api-client ← transport ← discovery-client).
      const { loadDiscoveryLookupId, proxiedTunnelUrl } = await import('./api-client')
      const lookupId = loadDiscoveryLookupId()
      if (!lookupId) return null
      const session = await lookupAny(lookupId)
      if (!session) return null
      // Prefer the worker-echoed discoveryId; fall back to the locally-stored
      // lookupId (which is the same value, persisted at pair time). Either
      // way, prefer the proxy URL whenever discovery is active — that's the
      // entire point of Phase 2.
      const id = session.discoveryId ?? lookupId
      const proxied = proxiedTunnelUrl(id)
      return proxied ?? session.tunnelUrl
    } finally {
      resolveInFlight = null
    }
  })()
  return resolveInFlight
}

async function rawLookup(key: string): Promise<LookupResult | null> {
  const base = discoveryBaseUrl()
  if (!base) return null
  try {
    const res = await fetch(`${base}/api/session/lookup?k=${encodeURIComponent(key)}`, {
      method: 'GET',
      credentials: 'omit',
    })
    if (!res.ok) return null
    const body = (await res.json()) as { tunnelUrl?: unknown; localIp?: unknown; discoveryId?: unknown }
    if (typeof body.tunnelUrl !== 'string' || body.tunnelUrl.length === 0) return null
    return {
      tunnelUrl: body.tunnelUrl.replace(/\/$/, ''),
      localIp: typeof body.localIp === 'string' ? body.localIp : null,
      discoveryId: typeof body.discoveryId === 'string' && body.discoveryId.length === 64
        ? body.discoveryId
        : null,
    }
  } catch {
    return null
  }
}
