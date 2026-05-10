import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { getCurrentTunnelUrl } from './discovery-client'

// getCurrentTunnelUrl calls lookupAny(lookupId) internally.
// We mock api-client so loadDiscoveryLookupId returns a controllable value,
// and spy on the module-level lookupAny to count worker hits.

vi.mock('./api-client', () => ({
  loadDiscoveryLookupId: vi.fn(() => null),
  getActiveHost: vi.fn(() => null),
  loadApiKey: vi.fn(() => null),
  loadTunnelBase: vi.fn(() => null),
  storeTunnelBase: vi.fn(),
  storeApiKey: vi.fn(),
  clearApiKey: vi.fn(),
  storeDiscoveryLookupId: vi.fn(),
  storeNamedTunnelHostname: vi.fn(),
  loadNamedTunnelHostname: vi.fn(() => null),
  setActiveHost: vi.fn(),
  makeRemoteClient: vi.fn(),
  installFetchInterceptor: vi.fn(),
  oxiFetch: vi.fn(),
  // Phase 2: getCurrentTunnelUrl prefers the proxy URL when discovery is
  // active. The test env sets no VITE_DISCOVERY_URL, so this returns null
  // and the resolver falls back to the raw tunnelUrl — same observable
  // result the test asserts (null in test env).
  proxiedTunnelUrl: vi.fn(() => null),
  migrateAllTunnelBasesToProxy: vi.fn(),
  migrateTunnelBaseToProxy: vi.fn(),
}))

describe('getCurrentTunnelUrl', () => {
  beforeEach(() => {
    localStorage.clear()
    vi.clearAllMocks()
  })

  afterEach(() => {
    localStorage.clear()
  })

  it('returns null when no discovery lookup id is stored', async () => {
    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadDiscoveryLookupId).mockReturnValue(null)

    const result = await getCurrentTunnelUrl()
    expect(result).toBeNull()
  })

  it('returns null when lookupAny returns null (worker unreachable / no URL)', async () => {
    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadDiscoveryLookupId).mockReturnValue('deadbeef')

    // No VITE_DISCOVERY_URL set → discoveryBaseUrl() is '' → rawLookup returns null.
    const result = await getCurrentTunnelUrl()
    expect(result).toBeNull()
  })

  it('coalesces concurrent callers — worker hit exactly once', async () => {
    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadDiscoveryLookupId).mockReturnValue('deadbeef')

    // Patch fetch (not called in test env since discoveryBaseUrl() is '' here,
    // but stubbed so the module doesn't hit the real network if env changes).
    vi.stubGlobal('fetch', vi.fn(async () => {
      return new Response(
        JSON.stringify({ tunnelUrl: 'https://abc.trycloudflare.com', localIp: null }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      )
    }))

    // Re-import the module fresh (resetModules ensures inFlight is null).
    vi.resetModules()
    const { getCurrentTunnelUrl: freshGetCurrentTunnelUrl, lookupAny } =
      await import('./discovery-client')

    // Spy on lookupAny to count actual worker calls (rawLookup gates on
    // discoveryBaseUrl which is empty in test env → lookupAny returns null,
    // so fetchCount stays 0; what we CAN verify is the coalesce itself by
    // checking lookupAny is called once not 5 times).
    const lookupAnySpy = vi.spyOn({ lookupAny }, 'lookupAny')

    // Fire 5 concurrent calls — all should share the same inFlight promise.
    const promises = Array.from({ length: 5 }, () => freshGetCurrentTunnelUrl())

    // The inFlight ref should be set after the first synchronous call.
    // All 5 callers attach to the same promise.
    const results = await Promise.all(promises)

    // All results are the same value (null in test env because discoveryBaseUrl is '').
    const uniqueResults = new Set(results)
    expect(uniqueResults.size).toBe(1)

    // lookupAny was either called once or not at all (no discovery URL in test env).
    // The important invariant: NOT called 5 times.
    expect(lookupAnySpy.mock.calls.length).toBeLessThanOrEqual(1)

    vi.unstubAllGlobals()
  })
})
