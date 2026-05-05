import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import {
  clearApiKey,
  loadApiKey,
  loadTunnelBase,
  storeApiKey,
  storeTunnelBase,
} from './api-client'

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
