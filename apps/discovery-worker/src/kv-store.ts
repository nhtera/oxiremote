export interface SessionRecord {
  tunnelUrl: string
  localIp?: string
  updatedAt: number
}

export interface KVLike {
  get(key: string): Promise<string | null>
  put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<void>
}

const SESSION_PREFIX = 'session:'
const TEMPKEY_PREFIX = 'tempkey:'
// Session record is the indirection target for every tempkey/code lookup.
// Must outlive the longest possible tempkey TTL (24h for permanent-key
// lookup_id) or stale `tempkey:` indexes will dangle and resolveTempKey()
// will return null even though the user-typed key is still valid. Agents
// idempotently overwrite via session/update on each TunnelUrlChanged + a
// 15-min heartbeat, so this is the cap, not the steady state.
export const SESSION_DEFAULT_TTL_SECS = 24 * 60 * 60
export const DEFAULT_TTL_SECS = 1800

export async function putSession(
  kv: KVLike,
  apiKeyHash: string,
  record: SessionRecord,
  ttlSecs: number = SESSION_DEFAULT_TTL_SECS,
): Promise<void> {
  await kv.put(SESSION_PREFIX + apiKeyHash, JSON.stringify(record), { expirationTtl: ttlSecs })
}

export async function getSession(kv: KVLike, apiKeyHash: string): Promise<SessionRecord | null> {
  const raw = await kv.get(SESSION_PREFIX + apiKeyHash)
  if (!raw) return null
  try {
    return JSON.parse(raw) as SessionRecord
  } catch {
    return null
  }
}

export async function putTempKey(
  kv: KVLike,
  tempKey: string,
  apiKeyHash: string,
  ttlSecs: number = DEFAULT_TTL_SECS,
): Promise<void> {
  await kv.put(TEMPKEY_PREFIX + tempKey, apiKeyHash, { expirationTtl: ttlSecs })
}

/**
 * Lookup result for `resolveTempKey`. Carries the session record AND the
 * discovery_id (== `apiKeyHash` in the existing schema) the tempkey/code
 * resolved to. The SPA needs the discovery_id to address the worker proxy
 * (`/proxy/<discovery_id>/...`); without it the SPA would have to round-
 * trip a second lookup, which is exactly the cross-origin DNS flow we
 * wired the proxy to skip.
 */
export interface TempKeyResolution {
  session: SessionRecord
  discoveryId: string
}

export async function resolveTempKey(
  kv: KVLike,
  tempKey: string,
): Promise<TempKeyResolution | null> {
  const apiKeyHash = await kv.get(TEMPKEY_PREFIX + tempKey)
  if (!apiKeyHash) return null
  const session = await getSession(kv, apiKeyHash)
  if (!session) return null
  return { session, discoveryId: apiKeyHash }
}

/**
 * Direct session lookup for the reverse-proxy route. The proxy receives a
 * raw discovery_id (no tempkey indirection) and needs the current
 * tunnelUrl. This is a thin wrapper around `getSession` with the
 * additional contract that an empty `tunnelUrl` is treated as "no
 * registered upstream" — the agent always writes a non-empty URL on
 * `session/update`, so an empty value means the session was created but
 * never populated and the proxy has no target to forward to.
 */
export async function resolveProxyTarget(
  kv: KVLike,
  discoveryId: string,
): Promise<SessionRecord | null> {
  const session = await getSession(kv, discoveryId)
  if (!session || !session.tunnelUrl) return null
  return session
}
