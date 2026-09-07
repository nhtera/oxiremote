import { describe, it, expect, beforeEach, vi } from 'vitest'
import { probeHost, refreshTunnelBaseFromDiscovery } from './host-reachability'

// The saved tunnel base in discovery mode is the worker proxy URL, so a
// health probe hits the Worker before it ever reaches the agent. These tests
// pin the rule that a response the Worker generated itself is never counted
// as proof the host is up — the bug was a long-dead host rendering "Online"
// because `404 {"error":"host_unknown"}` fell inside the 2xx-4xx band.

const PROXY_BASE = 'https://remote.example.dev/proxy/' + 'a'.repeat(64)

vi.mock('./api-client', () => ({
  loadTunnelBase: vi.fn(() => PROXY_BASE),
  storeTunnelBase: vi.fn(),
  loadDiscoveryLookupId: vi.fn(() => 'b'.repeat(64)),
  proxiedTunnelUrl: vi.fn((id: string) => `https://remote.example.dev/proxy/${id}`),
}))

vi.mock('./discovery-client', () => ({
  isDiscoveryMode: vi.fn(() => false),
  lookupAny: vi.fn(async () => null),
}))

function reply(status: number, body: string, headers: Record<string, string> = {}): Response {
  return new Response(status === 204 ? null : body, { status, headers })
}

function stubFetch(res: Response | Error) {
  const fn = vi.fn(async () => {
    if (res instanceof Error) throw res
    return res
  })
  vi.stubGlobal('fetch', fn)
  return fn
}

describe('probeHost classification', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('agent 200 → alive', async () => {
    stubFetch(reply(200, 'ok', { 'content-type': 'text/plain' }))
    expect(await probeHost('h1')).toBe('alive')
  })

  it('agent 401 → alive (key invalid, host is up)', async () => {
    stubFetch(reply(401, '{"error":"unauthorized"}', { 'content-type': 'application/json' }))
    expect(await probeHost('h1')).toBe('alive')
  })

  it('agent 404 (unknown route, no proxy marker) → alive', async () => {
    stubFetch(reply(404, 'not found', { 'content-type': 'text/plain' }))
    expect(await probeHost('h1')).toBe('alive')
  })

  it('proxy 404 host_unknown (header) → unreachable', async () => {
    stubFetch(
      reply(404, '{"error":"host_unknown"}', {
        'content-type': 'application/json',
        'x-oxi-proxy-error': 'host_unknown',
      }),
    )
    expect(await probeHost('h1')).toBe('unreachable')
  })

  it('proxy 404 host_unknown (body only, worker deployed before the header) → unreachable', async () => {
    stubFetch(reply(404, '{"error":"host_unknown"}', { 'content-type': 'application/json' }))
    expect(await probeHost('h1')).toBe('unreachable')
  })

  it('proxy 400 host_invalid → unreachable', async () => {
    stubFetch(
      reply(400, '{"error":"host_invalid"}', {
        'content-type': 'application/json',
        'x-oxi-proxy-error': 'host_invalid',
      }),
    )
    expect(await probeHost('h1')).toBe('unreachable')
  })

  it('proxy 429 rate_limited → unknown, not a red dot', async () => {
    stubFetch(
      reply(429, '{"error":"rate_limited"}', {
        'content-type': 'application/json',
        'x-oxi-proxy-error': 'rate_limited',
      }),
    )
    expect(await probeHost('h1')).toBe('unknown')
  })

  it('502 upstream_unreachable → unreachable', async () => {
    stubFetch(
      reply(502, '{"error":"upstream_unreachable"}', {
        'content-type': 'application/json',
        'x-oxi-proxy-error': 'upstream_unreachable',
      }),
    )
    expect(await probeHost('h1')).toBe('unreachable')
  })

  it('Cloudflare 530 → unreachable', async () => {
    stubFetch(reply(530, 'origin dns error', { 'content-type': 'text/plain' }))
    expect(await probeHost('h1')).toBe('unreachable')
  })

  it('network failure → unreachable', async () => {
    stubFetch(new TypeError('Failed to fetch'))
    expect(await probeHost('h1')).toBe('unreachable')
  })
})

describe('refreshTunnelBaseFromDiscovery', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('persists the worker proxy URL, not the raw quick-tunnel URL', async () => {
    const api = await import('./api-client')
    const disc = await import('./discovery-client')
    vi.mocked(disc.isDiscoveryMode).mockReturnValue(true)
    vi.mocked(disc.lookupAny).mockResolvedValue({
      tunnelUrl: 'https://abc-def.trycloudflare.com',
      localIp: null,
      discoveryId: 'c'.repeat(64),
    })

    const fresh = await refreshTunnelBaseFromDiscovery('h1')
    expect(fresh).toBe(`https://remote.example.dev/proxy/${'c'.repeat(64)}`)
    expect(vi.mocked(api.storeTunnelBase)).toHaveBeenCalledWith('h1', fresh)
  })

  it('falls back to the raw tunnel URL when no proxy URL can be built', async () => {
    const api = await import('./api-client')
    const disc = await import('./discovery-client')
    vi.mocked(disc.isDiscoveryMode).mockReturnValue(true)
    vi.mocked(api.proxiedTunnelUrl).mockReturnValue(null)
    vi.mocked(disc.lookupAny).mockResolvedValue({
      tunnelUrl: 'https://abc-def.trycloudflare.com/',
      localIp: null,
      discoveryId: null,
    })

    expect(await refreshTunnelBaseFromDiscovery('h1')).toBe('https://abc-def.trycloudflare.com')
  })
})
