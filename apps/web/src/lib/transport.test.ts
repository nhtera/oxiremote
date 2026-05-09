/**
 * Tests for transport.ts apiFetch stale-URL fallback.
 *
 * The retry path uses dynamic import() for api-client, discovery-client, and
 * url-validation. vi.mock hoists module mocks before dynamic imports resolve,
 * so all three modules are intercepted correctly.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { apiFetch, HostUnreachableError, SuspiciousTunnelUrlError } from './transport'

// ── Module mocks (hoisted) ───────────────────────────────────────────────────

vi.mock('./discovery-client', () => ({
  getCurrentTunnelUrl: vi.fn(async () => null),
  // Other exports used elsewhere — stub so the module resolves cleanly.
  discoveryBaseUrl: () => '',
  isDiscoveryMode: () => false,
  lookupAny: vi.fn(async () => null),
  lookupSession: vi.fn(async () => null),
  lookupPairingCode: vi.fn(async () => null),
  lookupPermanentKey: vi.fn(async () => null),
  isLikelyTempKey: () => false,
  isLikelyPairingCode: () => false,
  isLikelyOtk: () => false,
  isLikelyPermanentKey: () => false,
  permanentKeyLookupId: vi.fn(async () => ''),
}))

vi.mock('./api-client', () => ({
  getActiveHost: vi.fn(() => 'host1'),
  loadApiKey: vi.fn(() => null),
  loadTunnelBase: vi.fn(() => 'https://old.trycloudflare.com'),
  storeTunnelBase: vi.fn(),
  storeApiKey: vi.fn(),
  clearApiKey: vi.fn(),
  loadDiscoveryLookupId: vi.fn(() => null),
  storeDiscoveryLookupId: vi.fn(),
  storeNamedTunnelHostname: vi.fn(),
  loadNamedTunnelHostname: vi.fn(() => null),
  setActiveHost: vi.fn(),
  makeRemoteClient: vi.fn(),
  installFetchInterceptor: vi.fn(),
  oxiFetch: vi.fn(),
}))

vi.mock('./url-validation', () => ({
  isAllowedTunnelHost: vi.fn(() => true),
  getNamedTunnelAllowlist: vi.fn(() => []),
}))

// ── Helpers ─────────────────────────────────────────────────────────────────

function makeOkResponse(status = 200) {
  return new Response('ok', { status })
}

function make5xxResponse(status = 503) {
  return new Response('error', { status })
}

// Seed sessionStorage so getApiBase() returns a known base without a race.
function seedApiBase(base = 'https://old.trycloudflare.com') {
  sessionStorage.setItem('oxi_api_base', base)
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe('apiFetch', () => {
  beforeEach(async () => {
    sessionStorage.clear()
    localStorage.clear()
    vi.clearAllMocks()
    // Seed a stable API base so getApiBase() doesn't hit window.location.
    seedApiBase()

    // Reset per-test mock implementations to defaults.
    const discoveryMod = await import('./discovery-client')
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue(null)

    const apiMod = await import('./api-client')
    vi.mocked(apiMod.getActiveHost).mockReturnValue('host1')
    vi.mocked(apiMod.loadTunnelBase).mockReturnValue('https://old.trycloudflare.com')
    vi.mocked(apiMod.storeTunnelBase).mockReset()

    const validationMod = await import('./url-validation')
    vi.mocked(validationMod.isAllowedTunnelHost).mockReturnValue(true)
    vi.mocked(validationMod.getNamedTunnelAllowlist).mockReturnValue([])
  })

  afterEach(() => {
    sessionStorage.clear()
    localStorage.clear()
  })

  it('happy path — cache valid, no resolve called', async () => {
    const mockFetch = vi.fn(async () => makeOkResponse(200))
    vi.stubGlobal('fetch', mockFetch)

    const res = await apiFetch('/api/host')
    expect(res.status).toBe(200)

    // getCurrentTunnelUrl must NOT be called on a successful fetch.
    const { getCurrentTunnelUrl } = await import('./discovery-client')
    expect(vi.mocked(getCurrentTunnelUrl)).not.toHaveBeenCalled()

    vi.unstubAllGlobals()
  })

  it('stale URL resolves and retries — first fetch throws TypeError, retry succeeds', async () => {
    const discoveryMod = await import('./discovery-client')
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue('https://new.trycloudflare.com')

    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadTunnelBase).mockReturnValue('https://old.trycloudflare.com')

    const validationMod = await import('./url-validation')
    vi.mocked(validationMod.isAllowedTunnelHost).mockReturnValue(true)

    let callCount = 0
    const mockFetch = vi.fn(async () => {
      callCount++
      if (callCount === 1) throw new TypeError('Failed to fetch')
      return makeOkResponse(200)
    })
    vi.stubGlobal('fetch', mockFetch)

    const dispatchSpy = vi.spyOn(window, 'dispatchEvent')

    const res = await apiFetch('/api/workspace')
    expect(res.status).toBe(200)
    expect(callCount).toBe(2)

    // storeTunnelBase persists the new URL.
    expect(vi.mocked(apiMod.storeTunnelBase)).toHaveBeenCalledWith('host1', 'https://new.trycloudflare.com')

    // oxi:host-moved event was dispatched.
    expect(dispatchSpy).toHaveBeenCalledWith(expect.objectContaining({ type: 'oxi:host-moved' }))

    vi.unstubAllGlobals()
  })

  it('resolved URL outside allowlist is rejected — throws SuspiciousTunnelUrlError, original URL unchanged', async () => {
    const discoveryMod = await import('./discovery-client')
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue('https://evil.com')

    const validationMod = await import('./url-validation')
    vi.mocked(validationMod.isAllowedTunnelHost).mockReturnValue(false)

    const mockFetch = vi.fn(async () => { throw new TypeError('Failed to fetch') })
    vi.stubGlobal('fetch', mockFetch)

    const apiMod = await import('./api-client')

    await expect(apiFetch('/api/workspace')).rejects.toThrow(SuspiciousTunnelUrlError)

    // storeTunnelBase must NOT be called — original URL unchanged.
    expect(vi.mocked(apiMod.storeTunnelBase)).not.toHaveBeenCalled()

    vi.unstubAllGlobals()
  })

  it('resolve returns same URL — does not loop or retry', async () => {
    const discoveryMod = await import('./discovery-client')
    // Worker returns the SAME URL as what's already cached.
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue('https://old.trycloudflare.com')

    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadTunnelBase).mockReturnValue('https://old.trycloudflare.com')

    let callCount = 0
    const mockFetch = vi.fn(async () => {
      callCount++
      throw new TypeError('Failed to fetch')
    })
    vi.stubGlobal('fetch', mockFetch)

    await expect(apiFetch('/api/workspace')).rejects.toBeInstanceOf(HostUnreachableError)

    // fetch called only once — no retry because URL didn't change.
    expect(callCount).toBe(1)

    vi.unstubAllGlobals()
  })

  it('no discovery id — skips fallback and throws HostUnreachableError', async () => {
    const discoveryMod = await import('./discovery-client')
    // No lookup id → returns null without calling the worker.
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue(null)

    let callCount = 0
    const mockFetch = vi.fn(async () => {
      callCount++
      throw new TypeError('Failed to fetch')
    })
    vi.stubGlobal('fetch', mockFetch)

    await expect(apiFetch('/api/workspace')).rejects.toBeInstanceOf(HostUnreachableError)

    // fetch called only once — no retry.
    expect(callCount).toBe(1)

    vi.unstubAllGlobals()
  })

  it('5xx response triggers fallback and retries with new URL', async () => {
    const discoveryMod = await import('./discovery-client')
    vi.mocked(discoveryMod.getCurrentTunnelUrl).mockResolvedValue('https://new.trycloudflare.com')

    const apiMod = await import('./api-client')
    vi.mocked(apiMod.loadTunnelBase).mockReturnValue('https://old.trycloudflare.com')

    const validationMod = await import('./url-validation')
    vi.mocked(validationMod.isAllowedTunnelHost).mockReturnValue(true)

    let callCount = 0
    const mockFetch = vi.fn(async () => {
      callCount++
      if (callCount === 1) return make5xxResponse(503)
      return makeOkResponse(200)
    })
    vi.stubGlobal('fetch', mockFetch)

    const res = await apiFetch('/api/workspace')
    expect(res.status).toBe(200)
    expect(callCount).toBe(2)

    vi.unstubAllGlobals()
  })
})
