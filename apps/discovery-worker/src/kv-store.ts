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
export const DEFAULT_TTL_SECS = 1800

export async function putSession(
  kv: KVLike,
  apiKeyHash: string,
  record: SessionRecord,
  ttlSecs: number = DEFAULT_TTL_SECS,
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
