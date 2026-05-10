import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest'
import worker from '../src/worker'
import { _resetForTests as resetRateLimiter } from '../src/rate-limiter'
import type { KVLike } from '../src/kv-store'

// Loosely-typed fetch mock — the cf-workers `fetch` overload is hard to
// satisfy with vi.spyOn defaults; we only care about `.mock.calls`,
// `.mockResolvedValue`, `.mockImplementation`, `.mockRejectedValue`.
type FetchSpy = MockInstance<(input: unknown, init?: unknown) => Promise<Response>>

// Mock KV: same shape as `worker.test.ts`, kept tiny on purpose so the
// proxy tests don't drift from the production fixture.
class MockKV implements KVLike {
  private store = new Map<string, { value: string; expiresAt?: number }>()

  async get(key: string): Promise<string | null> {
    const e = this.store.get(key)
    if (!e) return null
    if (e.expiresAt !== undefined && e.expiresAt < Date.now()) {
      this.store.delete(key)
      return null
    }
    return e.value
  }

  async put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<void> {
    this.store.set(key, {
      value,
      expiresAt: opts?.expirationTtl !== undefined ? Date.now() + opts.expirationTtl * 1000 : undefined,
    })
  }
}

const ALLOWED_ORIGIN = 'https://remote.erai.dev'
const DISCOVERY_ID = 'a'.repeat(64)
const TUNNEL_URL = 'https://abc-def.trycloudflare.com'

function makeProxyReq(
  method: string,
  pathSuffix: string,
  opts: { origin?: string | null; body?: BodyInit; headers?: Record<string, string> } = {},
): Request {
  const headers: Record<string, string> = {
    'cf-connecting-ip': '1.2.3.4',
    ...opts.headers,
  }
  if (opts.origin !== null) headers['origin'] = opts.origin ?? ALLOWED_ORIGIN
  return new Request(`https://discovery.example.com/proxy/${DISCOVERY_ID}${pathSuffix}`, {
    method,
    headers,
    body: opts.body,
  })
}

async function seedSession(env: { DISCOVERY: MockKV }, tunnelUrl = TUNNEL_URL): Promise<void> {
  // Mirror what the agent does: POST /api/session/create then /api/session/update.
  const create = new Request('https://discovery.example.com/api/session/create', {
    method: 'POST',
    headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': '1.2.3.4' },
    body: JSON.stringify({ apiKey: DISCOVERY_ID }),
  })
  await worker.fetch(create, env)
  const update = new Request('https://discovery.example.com/api/session/update', {
    method: 'POST',
    headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': '1.2.3.4' },
    body: JSON.stringify({ apiKey: DISCOVERY_ID, tunnelUrl }),
  })
  await worker.fetch(update, env)
}

describe('proxy handler', () => {
  let env: { DISCOVERY: MockKV }
  let fetchSpy: FetchSpy

  beforeEach(() => {
    env = { DISCOVERY: new MockKV() }
    resetRateLimiter()
    fetchSpy = vi.spyOn(globalThis, 'fetch') as unknown as FetchSpy
  })

  afterEach(() => {
    fetchSpy.mockRestore()
  })

  it('400 on malformed hostKey', async () => {
    // Build a request that hits /proxy/<too-short>/... — bypass the helper
    // because `makeProxyReq` is hard-coded to the valid 64-hex id.
    const req = new Request('https://discovery.example.com/proxy/abcd/api/health', {
      method: 'GET',
      headers: { origin: ALLOWED_ORIGIN, 'cf-connecting-ip': '1.2.3.4' },
    })
    const r = await worker.fetch(req, env)
    // /proxy/abcd doesn't match HEX64 so the route regex misses → 404 not
    // found from the dispatcher (route doesn't match). Validates that the
    // route regex is the gate; handleProxy never sees malformed ids.
    expect(r.status).toBe(404)
  })

  it('400 on hostKey that is right length but not hex', async () => {
    // 64 chars but contains uppercase/digits-only — fails HEX64.
    const id = 'Z'.repeat(64)
    const req = new Request(`https://discovery.example.com/proxy/${id}/api/health`, {
      method: 'GET',
      headers: { origin: ALLOWED_ORIGIN, 'cf-connecting-ip': '1.2.3.4' },
    })
    const r = await worker.fetch(req, env)
    expect(r.status).toBe(404)
  })

  it('404 when discovery_id has no registered tunnel', async () => {
    fetchSpy.mockResolvedValue(new Response(null, { status: 200 }))
    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(404)
    expect(fetchSpy).not.toHaveBeenCalled()
    const body = (await r.json()) as { error: string }
    expect(body.error).toBe('host_unknown')
  })

  it('404 when session exists but tunnelUrl is empty', async () => {
    // Agent created the session but never called /update — treated as no
    // upstream by `resolveProxyTarget`.
    const create = new Request('https://discovery.example.com/api/session/create', {
      method: 'POST',
      headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': '1.2.3.4' },
      body: JSON.stringify({ apiKey: DISCOVERY_ID }),
    })
    await worker.fetch(create, env)
    fetchSpy.mockResolvedValue(new Response(null, { status: 200 }))

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(404)
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('200 forwards body + status when session exists', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(
      new Response(JSON.stringify({ ok: true, value: 42 }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(200)
    const body = (await r.json()) as { ok: boolean; value: number }
    expect(body).toEqual({ ok: true, value: 42 })
    expect(fetchSpy).toHaveBeenCalledTimes(1)

    // Upstream URL is the registered tunnelUrl + the suffix, no `/proxy/<id>` leak.
    const calledWith = fetchSpy.mock.calls[0]?.[0] as string
    expect(calledWith).toBe(`${TUNNEL_URL}/api/health`)
  })

  it('strips identifying headers before forwarding upstream', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))

    await worker.fetch(
      makeProxyReq('POST', '/api/login/one-time', {
        body: JSON.stringify({ token: 'abcd' }),
        headers: {
          'content-type': 'application/json',
          'cf-ray': 'should-be-stripped',
          'cf-connecting-ip': '1.2.3.4',
          authorization: 'Bearer should-pass',
          'x-oxi-csrf': 'csrf-should-pass',
          referer: 'should-be-stripped',
        },
      }),
      env,
    )

    const init = fetchSpy.mock.calls[0]?.[1] as RequestInit
    const fwdHeaders = init.headers as Headers
    expect(fwdHeaders.get('cf-ray')).toBeNull()
    expect(fwdHeaders.get('cf-connecting-ip')).toBeNull()
    expect(fwdHeaders.get('referer')).toBeNull()
    expect(fwdHeaders.get('origin')).toBeNull()
    // Auth + CSRF MUST pass through — they're how the agent authorizes
    // the upstream call.
    expect(fwdHeaders.get('authorization')).toBe('Bearer should-pass')
    expect(fwdHeaders.get('x-oxi-csrf')).toBe('csrf-should-pass')
    // Worker injects X-Forwarded-Host (= the worker hostname) for triage.
    expect(fwdHeaders.get('x-forwarded-host')).toBe('discovery.example.com')
  })

  it('CORS: ACAO matches request Origin and credentials enabled', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
    expect(r.headers.get('access-control-allow-credentials')).toBe('true')
    // Vary: Origin so caches don't pin a single ACAO across origins.
    expect(r.headers.get('vary')).toMatch(/origin/i)
  })

  it('CORS: rejects non-allowlisted origin with 403 (no ACAO leak)', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))

    const r = await worker.fetch(
      makeProxyReq('GET', '/api/health', { origin: 'https://evil.example.com' }),
      env,
    )
    expect(r.status).toBe(403)
    expect(r.headers.get('access-control-allow-origin')).toBeNull()
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('CORS: rejects missing Origin with 403', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))

    const r = await worker.fetch(makeProxyReq('GET', '/api/health', { origin: null }), env)
    expect(r.status).toBe(403)
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('OPTIONS preflight returns 204 + proxy CORS headers (Authorization, X-OXI-CSRF)', async () => {
    const r = await worker.fetch(makeProxyReq('OPTIONS', '/api/login/one-time'), env)
    expect(r.status).toBe(204)
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
    expect(r.headers.get('access-control-allow-headers')?.toLowerCase()).toContain('authorization')
    expect(r.headers.get('access-control-allow-headers')?.toLowerCase()).toContain('x-oxi-csrf')
    expect(r.headers.get('access-control-allow-credentials')).toBe('true')
  })

  it('drops upstream ACAO so it cannot fight worker CORS', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(
      new Response('ok', {
        status: 200,
        headers: {
          'access-control-allow-origin': 'https://evil.example.com',
          'access-control-allow-credentials': 'false',
        },
      }),
    )

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    // Worker overrides — the SPA only ever sees the worker's ACAO, never
    // an upstream-emitted one (which would point at the cross-origin agent).
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
    expect(r.headers.get('access-control-allow-credentials')).toBe('true')
  })

  it('5xx upstream is forwarded as-is with worker CORS attached', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('boom', { status: 503 }))

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(503)
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
  })

  it('502 when fetch throws (network/TLS failure)', async () => {
    await seedSession(env)
    fetchSpy.mockRejectedValue(new TypeError('connect ECONNREFUSED'))

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(502)
    const body = (await r.json()) as { error: string }
    expect(body.error).toBe('upstream_unreachable')
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
  })

  it('504 when fetch raises a TimeoutError (DOMException)', async () => {
    await seedSession(env)
    const timeoutErr = new DOMException('timed out', 'TimeoutError')
    fetchSpy.mockRejectedValue(timeoutErr)

    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(504)
    const body = (await r.json()) as { error: string }
    expect(body.error).toBe('upstream_timeout')
  })

  it('preserves Upgrade headers for WebSocket pass-through', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(
      new Response(null, {
        status: 101,
        headers: { upgrade: 'websocket', connection: 'Upgrade' },
      }),
    )

    const r = await worker.fetch(
      makeProxyReq('GET', '/ws/terminal/abc', {
        headers: {
          upgrade: 'websocket',
          connection: 'Upgrade',
          'sec-websocket-key': 'abc==',
          'sec-websocket-version': '13',
        },
      }),
      env,
    )
    expect(r.status).toBe(101)

    const init = fetchSpy.mock.calls[0]?.[1] as RequestInit
    const fwdHeaders = init.headers as Headers
    expect(fwdHeaders.get('upgrade')).toBe('websocket')
    expect(fwdHeaders.get('sec-websocket-key')).toBe('abc==')
    expect(fwdHeaders.get('sec-websocket-version')).toBe('13')
    // For 101 responses we MUST NOT re-wrap with a new Response — the body
    // is the live WS frame stream. Confirm we returned the upstream object
    // directly by checking that ACAO was NOT injected (worker doesn't
    // touch headers on the upgrade path).
    expect(r.headers.get('access-control-allow-origin')).toBeNull()
  })

  it('forwards query string verbatim', async () => {
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))

    await worker.fetch(makeProxyReq('GET', '/api/things?foo=bar&baz=qux'), env)
    const calledWith = fetchSpy.mock.calls[0]?.[0] as string
    expect(calledWith).toBe(`${TUNNEL_URL}/api/things?foo=bar&baz=qux`)
  })

  it('rate limit: 601st proxy req in same minute -> 429', async () => {
    await seedSession(env)
    // Fresh Response per call — re-using one Response across iterations
    // consumes its body stream on the first wrap, then throws
    // `ReadableStream has already been used` on the next.
    fetchSpy.mockImplementation(async () => new Response('ok', { status: 200 }))

    for (let i = 0; i < 600; i++) {
      const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
      expect(r.status).toBe(200)
    }
    const r = await worker.fetch(makeProxyReq('GET', '/api/health'), env)
    expect(r.status).toBe(429)
    expect(r.headers.get('access-control-allow-origin')).toBe(ALLOWED_ORIGIN)
  })

  it('proxy rate-limit bucket is isolated from /api/session/* control plane', async () => {
    // Hit the JSON API's /api/session/create with a different IP,
    // exhaust its 20/min quota, then confirm proxy from same IP still works
    // (different scope = different bucket).
    const ip = '7.7.7.7'
    for (let i = 0; i < 20; i++) {
      const r = new Request('https://discovery.example.com/api/session/create', {
        method: 'POST',
        headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': ip },
        body: JSON.stringify({ apiKey: DISCOVERY_ID }),
      })
      const res = await worker.fetch(r, env)
      expect(res.status).toBe(200)
    }
    // 21st JSON-API request on this IP is rate-limited.
    const blocked = new Request('https://discovery.example.com/api/session/create', {
      method: 'POST',
      headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': ip },
      body: JSON.stringify({ apiKey: DISCOVERY_ID }),
    })
    expect((await worker.fetch(blocked, env)).status).toBe(429)

    // Proxy request on same IP succeeds — different scope.
    await seedSession(env)
    fetchSpy.mockResolvedValue(new Response('ok', { status: 200 }))
    const proxyReq = new Request(`https://discovery.example.com/proxy/${DISCOVERY_ID}/api/health`, {
      method: 'GET',
      headers: { origin: ALLOWED_ORIGIN, 'cf-connecting-ip': ip },
    })
    expect((await worker.fetch(proxyReq, env)).status).toBe(200)
  })

  it('lookup response now includes discoveryId for the SPA', async () => {
    // Phase 1 worker change required for Phase 2 SPA: the SPA needs the
    // discovery_id alongside the tunnelUrl so it can address /proxy/<id>/.
    await seedSession(env)

    // Register a temp key to mimic the OTK flow.
    const tk = await worker.fetch(
      new Request('https://discovery.example.com/api/temp-key/create', {
        method: 'POST',
        headers: { origin: ALLOWED_ORIGIN, 'content-type': 'application/json', 'cf-connecting-ip': '1.2.3.4' },
        body: JSON.stringify({ apiKey: DISCOVERY_ID }),
      }),
      env,
    )
    const { tempKey } = (await tk.json()) as { tempKey: string }

    const lookup = await worker.fetch(
      new Request(`https://discovery.example.com/api/session/lookup?k=${tempKey}`, {
        method: 'GET',
        headers: { origin: ALLOWED_ORIGIN, 'cf-connecting-ip': '1.2.3.4' },
      }),
      env,
    )
    const body = (await lookup.json()) as {
      tunnelUrl: string
      localIp: string | null
      discoveryId: string
    }
    expect(body.tunnelUrl).toBe(TUNNEL_URL)
    expect(body.discoveryId).toBe(DISCOVERY_ID)
  })
})
