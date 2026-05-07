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
 *     200 { tunnelUrl: string }
 *     404 { error: 'not found' }   // unknown / expired
 *
 * `localIp` was removed in Phase 04 / H3 — the SPA was reaching the agent over
 * the public tunnel URL anyway, the LAN IP was never useful, and exposing it
 * leaked the operator's home network shape to anyone who could land a lookup.
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
    // Phase 05 / H2 (SPA-side): defence-in-depth scheme guard. The worker
    // already rejects non-https tunnelUrl writes, but a compromised or
    // misconfigured worker shouldn't be the only thing standing between the
    // SPA and an http:// downgrade. Parse + check protocol explicitly.
    let parsed: URL
    try {
      parsed = new URL(body.tunnelUrl)
    } catch {
      return null
    }
    if (parsed.protocol !== 'https:') return null
    return {
      tunnelUrl: body.tunnelUrl.replace(/\/$/, ''),
    }
  } catch {
    return null
  }
}
