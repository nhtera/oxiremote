import { beforeEach, describe, expect, it } from 'vitest'
import worker from '../src/worker'
import { _resetForTests as resetRateLimiter } from '../src/rate-limiter'
import type { KVLike } from '../src/kv-store'

class MockKV implements KVLike {
  private store = new Map<string, { value: string; expiresAt?: number }>()

  async get(key: string): Promise<string | null> {
    const entry = this.store.get(key)
    if (!entry) return null
    if (entry.expiresAt !== undefined && entry.expiresAt < Date.now()) {
      this.store.delete(key)
      return null
    }
    return entry.value
  }

  async put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<void> {
    this.store.set(key, {
      value,
      expiresAt: opts?.expirationTtl !== undefined ? Date.now() + opts.expirationTtl * 1000 : undefined,
    })
  }

  forceExpire(key: string): void {
    const entry = this.store.get(key)
    if (entry) entry.expiresAt = 1
  }
}

const ALLOWED_ORIGIN = 'https://oxiremote.app'
const HASH = 'a'.repeat(64)
const TUNNEL_URL = 'https://abc-def.trycloudflare.com'

function makeReq(method: string, path: string, body?: unknown, opts: { origin?: string | null; ip?: string } = {}): Request {
  const headers: Record<string, string> = {
    'cf-connecting-ip': opts.ip ?? '1.2.3.4',
    'content-type': 'application/json',
  }
  if (opts.origin !== null) headers['origin'] = opts.origin ?? ALLOWED_ORIGIN
  return new Request('https://discovery.example.com' + path, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

describe('discovery worker', () => {
  let env: { DISCOVERY: MockKV }

  beforeEach(() => {
    env = { DISCOVERY: new MockKV() }
    resetRateLimiter()
  })

  it('full round-trip: create -> update -> temp-key -> lookup', async () => {
    const create = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    expect(create.status).toBe(200)

    const update = await worker.fetch(
      makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL, localIp: '192.168.1.10:8787' }),
      env,
    )
    expect(update.status).toBe(200)

    const tk = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
    expect(tk.status).toBe(200)
    const tkBody = (await tk.json()) as { tempKey: string; expiresAt: number }
    expect(tkBody.tempKey).toMatch(/^[a-f0-9]{32}$/)
    expect(tkBody.expiresAt).toBeGreaterThan(Date.now())

    const lookup = await worker.fetch(makeReq('GET', `/api/session/lookup?k=${tkBody.tempKey}`), env)
    expect(lookup.status).toBe(200)
    const result = (await lookup.json()) as { tunnelUrl: string; localIp: string | null }
    expect(result.tunnelUrl).toBe(TUNNEL_URL)
    expect(result.localIp).toBe('192.168.1.10:8787')
  })

  it('lookup unknown key -> 404', async () => {
    const r = await worker.fetch(makeReq('GET', '/api/session/lookup?k=deadbeefdeadbeefdeadbeefdeadbeef'), env)
    expect(r.status).toBe(404)
  })

  it('lookup expired temp key -> 404', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
    const tk = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
    const { tempKey } = (await tk.json()) as { tempKey: string }
    env.DISCOVERY.forceExpire(`tempkey:${tempKey}`)

    const lookup = await worker.fetch(makeReq('GET', `/api/session/lookup?k=${tempKey}`), env)
    expect(lookup.status).toBe(404)
  })

  it('temp-key create without prior session -> 404', async () => {
    const r = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
    expect(r.status).toBe(404)
  })

  it('CORS rejection from non-allowlisted origin on POST -> 403', async () => {
    const r = await worker.fetch(
      makeReq('POST', '/api/session/create', { apiKey: HASH }, { origin: 'https://evil.example.com' }),
      env,
    )
    expect(r.status).toBe(403)
  })

  it('CORS allows *.pages.dev origin', async () => {
    const r = await worker.fetch(
      makeReq('POST', '/api/session/create', { apiKey: HASH }, { origin: 'https://oxiremote.pages.dev' }),
      env,
    )
    expect(r.status).toBe(200)
    expect(r.headers.get('access-control-allow-origin')).toBe('https://oxiremote.pages.dev')
  })

  it('OPTIONS preflight -> 204 with CORS headers', async () => {
    const r = await worker.fetch(makeReq('OPTIONS', '/api/session/create'), env)
    expect(r.status).toBe(204)
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
    expect(r.headers.get('access-control-allow-methods')).toContain('POST')
  })

  it('invalid apiKey -> 400', async () => {
    const r = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: 'not-a-hex-hash' }), env)
    expect(r.status).toBe(400)
  })

  it('rate limit: 21st POST from same IP in same minute -> 429', async () => {
    for (let i = 0; i < 20; i++) {
      const r = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }, { ip: '9.9.9.9' }), env)
      expect(r.status).toBe(200)
    }
    const r = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }, { ip: '9.9.9.9' }), env)
    expect(r.status).toBe(429)
  })

  it('GET routes are exempt from rate limiter', async () => {
    for (let i = 0; i < 25; i++) {
      const r = await worker.fetch(makeReq('GET', '/api/session/lookup?k=x', undefined, { ip: '8.8.8.8' }), env)
      expect(r.status).toBe(404)
    }
  })

  it('unknown route -> 404', async () => {
    const r = await worker.fetch(makeReq('GET', '/api/nope'), env)
    expect(r.status).toBe(404)
  })
})
