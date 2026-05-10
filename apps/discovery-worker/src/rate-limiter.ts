// Best-effort in-memory per-IP rate limiter. State is per-isolate and resets
// on worker restart — acceptable for free-tier discovery; KV-backed limiter
// deferred to v2.
//
// `scope` namespaces the bucket so traffic classes don't share budget. The
// JSON-API control plane (`/api/session/*`) keeps the original 20/min cap;
// the reverse proxy (`/proxy/<id>/...`) gets a higher cap because a single
// pair flow + a couple of WS handshakes plus the eventual authed calls can
// easily exceed 20 reqs/min from one IP.

const buckets = new Map<string, { count: number; bucket: number }>()
const DEFAULT_MAX_PER_MIN = 20
const MAX_TRACKED_IPS = 1024

export function allow(
  ip: string,
  scope: string = '',
  maxPerMin: number = DEFAULT_MAX_PER_MIN,
): boolean {
  const key = scope ? `${scope}:${ip}` : ip
  const bucket = Math.floor(Date.now() / 60_000)
  const entry = buckets.get(key)
  if (!entry || entry.bucket !== bucket) {
    if (buckets.size >= MAX_TRACKED_IPS) evictStale(bucket)
    buckets.set(key, { count: 1, bucket })
    return true
  }
  entry.count += 1
  return entry.count <= maxPerMin
}

function evictStale(currentBucket: number): void {
  for (const [ip, entry] of buckets) {
    if (entry.bucket < currentBucket) buckets.delete(ip)
  }
}

export function _resetForTests(): void {
  buckets.clear()
}
