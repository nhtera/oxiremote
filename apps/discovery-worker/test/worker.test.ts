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

  async delete(key: string): Promise<void> {
    this.store.delete(key)
  }

  forceExpire(key: string): void {
    const entry = this.store.get(key)
    if (entry) entry.expiresAt = 1
  }
}

const ALLOWED_ORIGIN = 'https://oxiremote.app'
const HASH = 'a'.repeat(64)
const TUNNEL_URL = 'https://abc-def.trycloudflare.com'
const AGENT_SECRET = 'test-agent-secret-32-chars-long-aaaa'

function makeReq(
  method: string,
  path: string,
  body?: unknown,
  opts: { origin?: string | null; ip?: string; auth?: string | false } = {},
): Request {
  const headers: Record<string, string> = {
    'cf-connecting-ip': opts.ip ?? '1.2.3.4',
    'content-type': 'application/json',
  }
  if (opts.origin !== null) headers['origin'] = opts.origin ?? ALLOWED_ORIGIN
  // Default to attaching the canonical Bearer header so the bulk of tests
  // exercise the happy path. Pass `auth: false` to omit; pass a string to
  // override (e.g. "Bearer wrong" or "Basic ...").
  if (opts.auth !== false) {
    headers['authorization'] = opts.auth ?? `Bearer ${AGENT_SECRET}`
  }
  return new Request('https://discovery.example.com' + path, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

function makeEnv(): { DISCOVERY: MockKV; AGENT_REGISTRATION_SECRET: string } {
  return { DISCOVERY: new MockKV(), AGENT_REGISTRATION_SECRET: AGENT_SECRET }
}

describe('discovery worker', () => {
  let env: { DISCOVERY: MockKV; AGENT_REGISTRATION_SECRET?: string }

  beforeEach(() => {
    env = makeEnv()
    resetRateLimiter()
  })

  it('full round-trip: create -> update -> temp-key -> lookup', async () => {
    const create = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    expect(create.status).toBe(200)

    const update = await worker.fetch(
      makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }),
      env,
    )
    expect(update.status).toBe(200)

    const tk = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
    expect(tk.status).toBe(200)
    const tkBody = (await tk.json()) as { tempKey: string; expiresAt: number }
    expect(tkBody.tempKey).toMatch(/^[a-f0-9]{32}$/)
    expect(tkBody.expiresAt).toBeGreaterThan(Date.now())

    const lookup = await worker.fetch(makeReq('GET', `/api/session/lookup?k=${tkBody.tempKey}`, undefined, { auth: false }), env)
    expect(lookup.status).toBe(200)
    const result = (await lookup.json()) as { tunnelUrl: string }
    expect(result.tunnelUrl).toBe(TUNNEL_URL)
    // Phase 04 / H3 — localIp must not appear in the response payload.
    expect(result).not.toHaveProperty('localIp')
  })

  it('lookup unknown key -> 404', async () => {
    const r = await worker.fetch(
      makeReq('GET', '/api/session/lookup?k=deadbeefdeadbeefdeadbeefdeadbeef', undefined, { auth: false }),
      env,
    )
    expect(r.status).toBe(404)
  })

  it('lookup expired temp key -> 404', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
    const tk = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
    const { tempKey } = (await tk.json()) as { tempKey: string }
    env.DISCOVERY.forceExpire(`tempkey:${tempKey}`)

    const lookup = await worker.fetch(makeReq('GET', `/api/session/lookup?k=${tempKey}`, undefined, { auth: false }), env)
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
    const r = await worker.fetch(makeReq('OPTIONS', '/api/session/create', undefined, { auth: false }), env)
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

  // Phase 04 / H3 — lookup is now in the rate-limited path set.
  it('rate limit: 21st GET /api/session/lookup from same IP in same minute -> 429', async () => {
    for (let i = 0; i < 20; i++) {
      const r = await worker.fetch(
        makeReq('GET', '/api/session/lookup?k=x', undefined, { ip: '8.8.8.8', auth: false }),
        env,
      )
      expect(r.status, `expected 400/404 on call ${i + 1}`).toBeLessThan(429)
    }
    const r = await worker.fetch(
      makeReq('GET', '/api/session/lookup?k=x', undefined, { ip: '8.8.8.8', auth: false }),
      env,
    )
    expect(r.status).toBe(429)
  })

  it('unknown route -> 404', async () => {
    const r = await worker.fetch(makeReq('GET', '/api/nope', undefined, { auth: false }), env)
    expect(r.status).toBe(404)
  })

  it('code/register: round-trip lookup resolves to the agent tunnel', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)

    const reg = await worker.fetch(
      makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'ABCD1234' }),
      env,
    )
    expect(reg.status).toBe(200)

    const lookup = await worker.fetch(makeReq('GET', '/api/session/lookup?k=ABCD1234', undefined, { auth: false }), env)
    expect(lookup.status).toBe(200)
    const body = (await lookup.json()) as { tunnelUrl: string }
    expect(body.tunnelUrl).toBe(TUNNEL_URL)
  })

  it('code/register: rejects invalid code shape', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
    // Reject: too-short, contains punctuation, contains whitespace, > 32 chars.
    for (const bad of ['TOO', 'has-dash', 'has space', '!!@@##', 'a'.repeat(33)]) {
      const r = await worker.fetch(
        makeReq('POST', '/api/code/register', { apiKey: HASH, code: bad }),
        env,
      )
      expect(r.status, `expected 400 for ${JSON.stringify(bad)}`).toBe(400)
    }
  })

  it('code/register: accepts both pairing codes and OTKs (case-mixed alnum 6-32)', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
    for (const ok of ['ABCD1234', 'racuro4t3e6sgqy6', 'Mixed123Case', 'aaaaaaaa']) {
      const r = await worker.fetch(
        makeReq('POST', '/api/code/register', { apiKey: HASH, code: ok }),
        env,
      )
      expect(r.status, `expected 200 for ${JSON.stringify(ok)}`).toBe(200)
      const lookup = await worker.fetch(makeReq('GET', `/api/session/lookup?k=${ok}`, undefined, { auth: false }), env)
      expect(lookup.status).toBe(200)
    }
  })

  it('code/register: 404 when no prior session', async () => {
    const r = await worker.fetch(
      makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'XYZ12345' }),
      env,
    )
    expect(r.status).toBe(404)
  })

  it('code/register: clamps oversized TTL to default', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)

    // 30 days exceeds the 24h ceiling -> falls back to the 5-min default.
    const r = await worker.fetch(
      makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'CODE5678', expiryMinutes: 60 * 24 * 30 }),
      env,
    )
    expect(r.status).toBe(200)
    const body = (await r.json()) as { expiresAt: number }
    const ttlSecs = Math.round((body.expiresAt - Date.now()) / 1000)
    expect(ttlSecs).toBeGreaterThan(0)
    expect(ttlSecs).toBeLessThanOrEqual(5 * 60 + 5) // tolerate +5s clock skew
  })

  it('code/register: accepts 24h TTL for permanent-key lookup_ids', async () => {
    await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)

    const r = await worker.fetch(
      makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'deadbeefcafebabe', expiryMinutes: 60 * 24 }),
      env,
    )
    expect(r.status).toBe(200)
    const body = (await r.json()) as { expiresAt: number }
    const ttlSecs = Math.round((body.expiresAt - Date.now()) / 1000)
    expect(ttlSecs).toBeGreaterThan(60 * 60 * 23) // ~24 h, allow a few seconds of slop
  })

  // ─────────────────────────────────────────────────────────────────────
  // Phase 04 / H1 — Bearer guard
  // ─────────────────────────────────────────────────────────────────────

  describe('Bearer auth on mutating routes', () => {
    it('missing Authorization -> 401 on every mutating route', async () => {
      const routes: Array<[string, unknown]> = [
        ['/api/session/create', { apiKey: HASH }],
        ['/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }],
        ['/api/temp-key/create', { apiKey: HASH }],
        ['/api/code/register', { apiKey: HASH, code: 'ABCD1234' }],
      ]
      for (const [path, body] of routes) {
        const r = await worker.fetch(makeReq('POST', path, body, { auth: false }), env)
        expect(r.status, `${path} should require Bearer`).toBe(401)
      }
    })

    it('wrong Bearer secret -> 401', async () => {
      const r = await worker.fetch(
        makeReq('POST', '/api/session/create', { apiKey: HASH }, { auth: 'Bearer wrong-secret' }),
        env,
      )
      expect(r.status).toBe(401)
    })

    it('malformed Authorization header -> 401', async () => {
      for (const bad of ['', 'Basic abc', 'Bearer', 'Bearer  ', `Token ${AGENT_SECRET}`]) {
        const r = await worker.fetch(
          makeReq('POST', '/api/session/create', { apiKey: HASH }, { auth: bad }),
          env,
        )
        expect(r.status, `auth=${JSON.stringify(bad)}`).toBe(401)
      }
    })

    it('AGENT_REGISTRATION_SECRET unset -> 503 fail-closed', async () => {
      const e: { DISCOVERY: MockKV; AGENT_REGISTRATION_SECRET?: string } = makeEnv()
      delete e.AGENT_REGISTRATION_SECRET
      const r = await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), e)
      expect(r.status).toBe(503)
    })

    it('valid Bearer is checked AFTER rate-limit (defence in depth)', async () => {
      // Burn the rate-limit bucket with valid auth, then a *wrong*-auth request
      // should return 429 (rate limit), not 401 — the limiter runs first.
      for (let i = 0; i < 20; i++) {
        const r = await worker.fetch(
          makeReq('POST', '/api/session/create', { apiKey: HASH }, { ip: '7.7.7.7' }),
          env,
        )
        expect(r.status).toBe(200)
      }
      const r = await worker.fetch(
        makeReq(
          'POST',
          '/api/session/create',
          { apiKey: HASH },
          { ip: '7.7.7.7', auth: 'Bearer wrong-secret' },
        ),
        env,
      )
      expect(r.status).toBe(429)
    })

    it('lookup endpoint stays open without Bearer', async () => {
      // The SPA lookup path has no secret to send; it must remain anonymous.
      await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
      await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
      await worker.fetch(makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'ABCD1234' }), env)

      const r = await worker.fetch(
        makeReq('GET', '/api/session/lookup?k=ABCD1234', undefined, { auth: false }),
        env,
      )
      expect(r.status).toBe(200)
    })
  })

  // ─────────────────────────────────────────────────────────────────────
  // Phase 04 / H2 — tunnelUrl scheme guard
  // ─────────────────────────────────────────────────────────────────────

  describe('tunnelUrl scheme guard', () => {
    beforeEach(async () => {
      await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
    })

    it('accepts https://… URLs', async () => {
      const r = await worker.fetch(
        makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: 'https://example.trycloudflare.com' }),
        env,
      )
      expect(r.status).toBe(200)
    })

    it('rejects http://… URLs', async () => {
      const r = await worker.fetch(
        makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: 'http://example.com' }),
        env,
      )
      expect(r.status).toBe(400)
    })

    it('rejects javascript: and data: schemes', async () => {
      for (const bad of ['javascript:alert(1)', 'data:text/html,<script>alert(1)</script>', 'file:///etc/passwd']) {
        const r = await worker.fetch(
          makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: bad }),
          env,
        )
        expect(r.status, `expected 400 for ${JSON.stringify(bad)}`).toBe(400)
      }
    })

    it('rejects garbage / non-URL strings', async () => {
      for (const bad of ['not-a-url', '://nope', '   ', 'https//missing-colon.example']) {
        const r = await worker.fetch(
          makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: bad }),
          env,
        )
        expect(r.status, `expected 400 for ${JSON.stringify(bad)}`).toBe(400)
      }
    })
  })

  // ─────────────────────────────────────────────────────────────────────
  // Phase 04 / H4 — delete-on-read for machine temp keys
  // ─────────────────────────────────────────────────────────────────────

  describe('delete-on-read', () => {
    beforeEach(async () => {
      await worker.fetch(makeReq('POST', '/api/session/create', { apiKey: HASH }), env)
      await worker.fetch(makeReq('POST', '/api/session/update', { apiKey: HASH, tunnelUrl: TUNNEL_URL }), env)
    })

    it('machine 32-hex temp key is single-use', async () => {
      const tk = await worker.fetch(makeReq('POST', '/api/temp-key/create', { apiKey: HASH }), env)
      const { tempKey } = (await tk.json()) as { tempKey: string }

      const first = await worker.fetch(
        makeReq('GET', `/api/session/lookup?k=${tempKey}`, undefined, { auth: false }),
        env,
      )
      expect(first.status).toBe(200)

      const second = await worker.fetch(
        makeReq('GET', `/api/session/lookup?k=${tempKey}`, undefined, { auth: false }),
        env,
      )
      expect(second.status).toBe(404)
    })

    it('pairing code (8 alnum upper) survives multiple lookups', async () => {
      await worker.fetch(makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'ABCD1234' }), env)
      for (let i = 0; i < 3; i++) {
        const r = await worker.fetch(
          makeReq('GET', '/api/session/lookup?k=ABCD1234', undefined, { auth: false }),
          env,
        )
        expect(r.status, `lookup #${i + 1}`).toBe(200)
      }
    })

    it('OTK (16 lowercase alnum, but not pure hex) survives multiple lookups', async () => {
      // racuro4t3e6sgqy6 contains 'r','c','u','o','s','q','y' — not 0-9a-f.
      await worker.fetch(makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'racuro4t3e6sgqy6' }), env)
      for (let i = 0; i < 3; i++) {
        const r = await worker.fetch(
          makeReq('GET', '/api/session/lookup?k=racuro4t3e6sgqy6', undefined, { auth: false }),
          env,
        )
        expect(r.status, `lookup #${i + 1}`).toBe(200)
      }
    })

    it('permanent-key lookup_id (16 hex) survives — shape is too short to match machine 32-hex', async () => {
      // The agent's permanent-key lookup_id is exactly 16 hex chars (SHA-256
      // truncated). It must NOT be deleted on read so multiple SPA tabs can
      // resolve the same sk-… within the 24h TTL.
      await worker.fetch(makeReq('POST', '/api/code/register', { apiKey: HASH, code: 'deadbeefcafebabe' }), env)
      for (let i = 0; i < 3; i++) {
        const r = await worker.fetch(
          makeReq('GET', '/api/session/lookup?k=deadbeefcafebabe', undefined, { auth: false }),
          env,
        )
        expect(r.status, `lookup #${i + 1}`).toBe(200)
      }
    })
  })
})
