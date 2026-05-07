import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// Stub the discovery base URL so rawLookup actually runs the fetch path.
// vitest puts `import.meta.env.DEV` in the same shape Vite does — discovery is
// gated off in dev to keep the local proxy path intact, so we have to flip
// both flags before each test.
const ENV = import.meta.env as unknown as Record<string, string | boolean | undefined>
const ORIGINAL_DISCOVERY = ENV.VITE_DISCOVERY_URL
const ORIGINAL_DEV = ENV.DEV

beforeEach(() => {
  ENV.VITE_DISCOVERY_URL = 'https://oxi-discovery.test.workers.dev'
  ENV.DEV = false
})

afterEach(() => {
  ENV.VITE_DISCOVERY_URL = ORIGINAL_DISCOVERY
  ENV.DEV = ORIGINAL_DEV
  vi.restoreAllMocks()
})

function mockFetchOnce(body: unknown, ok = true) {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValueOnce({
      ok,
      json: () => Promise.resolve(body),
    } as Response),
  )
}

describe('rawLookup tunnelUrl scheme guard (Phase 05 / H2 SPA)', () => {
  it('returns the result for an https tunnelUrl', async () => {
    mockFetchOnce({ tunnelUrl: 'https://abc.trycloudflare.com' })
    const { lookupAny } = await import('./discovery-client')
    const r = await lookupAny('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')
    expect(r).not.toBeNull()
    expect(r!.tunnelUrl).toBe('https://abc.trycloudflare.com')
  })

  it('rejects http:// tunnelUrl from a tampered worker response', async () => {
    mockFetchOnce({ tunnelUrl: 'http://attacker.example.com' })
    const { lookupAny } = await import('./discovery-client')
    expect(await lookupAny('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')).toBeNull()
  })

  it('rejects javascript: pseudo-scheme', async () => {
    mockFetchOnce({ tunnelUrl: 'javascript:alert(1)' })
    const { lookupAny } = await import('./discovery-client')
    expect(await lookupAny('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')).toBeNull()
  })

  it('rejects unparseable garbage', async () => {
    mockFetchOnce({ tunnelUrl: 'not a url at all' })
    const { lookupAny } = await import('./discovery-client')
    expect(await lookupAny('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')).toBeNull()
  })

  it('strips trailing slash on accepted https tunnelUrl', async () => {
    mockFetchOnce({ tunnelUrl: 'https://abc.trycloudflare.com/' })
    const { lookupAny } = await import('./discovery-client')
    const r = await lookupAny('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')
    expect(r!.tunnelUrl).toBe('https://abc.trycloudflare.com')
  })
})
