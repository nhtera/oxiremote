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

export type LookupResult = {
  tunnelUrl: string
  localIp: string | null
}

/** Discovery worker base URL injected at SPA build time. Empty / unset means
 *  embedded mode — callers must check `isDiscoveryMode()` before calling
 *  `lookupSession`.
 */
export function discoveryBaseUrl(): string {
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

async function rawLookup(key: string): Promise<LookupResult | null> {
  const base = discoveryBaseUrl()
  if (!base) return null
  try {
    const res = await fetch(`${base}/api/session/lookup?k=${encodeURIComponent(key)}`, {
      method: 'GET',
      credentials: 'omit',
    })
    if (!res.ok) return null
    const body = (await res.json()) as Partial<LookupResult>
    if (typeof body.tunnelUrl !== 'string' || body.tunnelUrl.length === 0) return null
    return {
      tunnelUrl: body.tunnelUrl.replace(/\/$/, ''),
      localIp: typeof body.localIp === 'string' ? body.localIp : null,
    }
  } catch {
    return null
  }
}
