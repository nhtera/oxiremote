import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import {
  clearApiKey,
  loadApiKey,
  loadTunnelBase,
  migrateAllTunnelBasesToProxy,
  migrateTunnelBaseToProxy,
  proxiedTunnelUrl,
  storeApiKey,
  storeDiscoveryLookupId,
  storeTunnelBase,
} from './api-client'

// Phase 2 helpers exercise `discoveryBaseUrl()` from discovery-client. The
// production module reads `import.meta.env.VITE_DISCOVERY_URL` which is
// empty in test runs — mock it to a stable URL so the proxy-URL builder
// has something to concatenate against.
vi.mock('./discovery-client', () => ({
  discoveryBaseUrl: () => 'https://oxiremote-discovery.test.workers.dev',
  isDiscoveryMode: () => true,
}))

beforeEach(() => localStorage.clear())
afterEach(() => localStorage.clear())

describe('clearApiKey', () => {
  it('removes both api key and tunnel base for a single host', () => {
    storeApiKey('host1', 'key123')
    storeTunnelBase('host1', 'https://a.tunnel.cf')
    clearApiKey('host1')
    expect(loadApiKey('host1')).toBeNull()
    expect(loadTunnelBase('host1')).toBeNull()
  })

  it('without hostId clears every api key and tunnel base', () => {
    storeApiKey('host1', 'k1')
    storeApiKey('host2', 'k2')
    storeTunnelBase('host1', 'https://a.tunnel.cf')
    storeTunnelBase('host2', 'https://b.tunnel.cf')
    clearApiKey()
    expect(loadApiKey('host1')).toBeNull()
    expect(loadApiKey('host2')).toBeNull()
    expect(loadTunnelBase('host1')).toBeNull()
    expect(loadTunnelBase('host2')).toBeNull()
  })

  it('leaves unrelated localStorage keys untouched', () => {
    localStorage.setItem('oxi:saved-hosts', '[]')
    storeApiKey('host1', 'k1')
    clearApiKey('host1')
    expect(localStorage.getItem('oxi:saved-hosts')).toBe('[]')
  })
})

describe('loadTunnelBase', () => {
  it('falls back to active host when no hostId is passed', () => {
    storeApiKey('host1', 'k1')
    storeTunnelBase('host1', 'https://a.tunnel.cf')
    expect(loadTunnelBase()).toBe('https://a.tunnel.cf')
  })

  it('returns null when nothing is stored', () => {
    expect(loadTunnelBase('unknown')).toBeNull()
  })

  it('strips a trailing slash on store', () => {
    storeTunnelBase('host1', 'https://a.tunnel.cf/')
    expect(loadTunnelBase('host1')).toBe('https://a.tunnel.cf')
  })
})

describe('proxiedTunnelUrl', () => {
  it('builds /proxy/<id> against the configured worker base', () => {
    expect(proxiedTunnelUrl('a'.repeat(64))).toBe(
      `https://oxiremote-discovery.test.workers.dev/proxy/${'a'.repeat(64)}`,
    )
  })

  it('strips trailing slash on the supplied base', () => {
    expect(proxiedTunnelUrl('deadbeef', 'https://example.dev/')).toBe(
      'https://example.dev/proxy/deadbeef',
    )
  })

  it('returns null when no worker base is configured', () => {
    expect(proxiedTunnelUrl('id', '')).toBeNull()
  })
})

describe('migrateTunnelBaseToProxy', () => {
  const HOST = 'host-mig'
  const DISCOVERY = 'b'.repeat(64)

  it('rewrites a *.trycloudflare.com base to the proxy URL', () => {
    storeTunnelBase(HOST, 'https://abc-def.trycloudflare.com')
    storeDiscoveryLookupId(HOST, DISCOVERY)

    const next = migrateTunnelBaseToProxy(HOST)
    expect(next).toBe(
      `https://oxiremote-discovery.test.workers.dev/proxy/${DISCOVERY}`,
    )
    expect(loadTunnelBase(HOST)).toBe(next)
  })

  it('is a no-op when the cached base is not *.trycloudflare.com', () => {
    storeTunnelBase(HOST, 'https://oxiremote.example.com')
    storeDiscoveryLookupId(HOST, DISCOVERY)

    expect(migrateTunnelBaseToProxy(HOST)).toBeNull()
    expect(loadTunnelBase(HOST)).toBe('https://oxiremote.example.com')
  })

  it('is a no-op when no discovery_id is stored (pre-0.1.28 pair)', () => {
    storeTunnelBase(HOST, 'https://abc.trycloudflare.com')
    expect(migrateTunnelBaseToProxy(HOST)).toBeNull()
    expect(loadTunnelBase(HOST)).toBe('https://abc.trycloudflare.com')
  })

  it('migrateAllTunnelBasesToProxy walks every TUNNEL_BASE_PREFIX entry', () => {
    storeTunnelBase('h1', 'https://one.trycloudflare.com')
    storeDiscoveryLookupId('h1', 'a'.repeat(64))
    storeTunnelBase('h2', 'https://two.trycloudflare.com')
    storeDiscoveryLookupId('h2', 'b'.repeat(64))
    storeTunnelBase('h3', 'https://named.example.com')
    storeDiscoveryLookupId('h3', 'c'.repeat(64))

    migrateAllTunnelBasesToProxy()

    expect(loadTunnelBase('h1')).toMatch(/\/proxy\/a{64}$/)
    expect(loadTunnelBase('h2')).toMatch(/\/proxy\/b{64}$/)
    // Named tunnel — left alone (only *.trycloudflare.com is migrated).
    expect(loadTunnelBase('h3')).toBe('https://named.example.com')
  })
})
