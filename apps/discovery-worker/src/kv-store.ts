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

export async function resolveTempKey(kv: KVLike, tempKey: string): Promise<SessionRecord | null> {
  const apiKeyHash = await kv.get(TEMPKEY_PREFIX + tempKey)
  if (!apiKeyHash) return null
  return getSession(kv, apiKeyHash)
}
