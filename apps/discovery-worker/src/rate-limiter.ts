// Best-effort in-memory per-IP rate limiter. State is per-isolate and resets
// on worker restart — acceptable for free-tier discovery; KV-backed limiter
// deferred to v2.

const buckets = new Map<string, { count: number; bucket: number }>()
const MAX_PER_MIN = 20
const MAX_TRACKED_IPS = 1024

export function allow(ip: string): boolean {
  const bucket = Math.floor(Date.now() / 60_000)
  const entry = buckets.get(ip)
  if (!entry || entry.bucket !== bucket) {
    if (buckets.size >= MAX_TRACKED_IPS) evictStale(bucket)
    buckets.set(ip, { count: 1, bucket })
    return true
  }
  entry.count += 1
  return entry.count <= MAX_PER_MIN
}

function evictStale(currentBucket: number): void {
  for (const [ip, entry] of buckets) {
    if (entry.bucket < currentBucket) buckets.delete(ip)
  }
}

export function _resetForTests(): void {
  buckets.clear()
}
