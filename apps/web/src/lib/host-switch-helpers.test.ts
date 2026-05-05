import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { switchActiveHost } from './host-switch-helpers'
import { storeApiKey, storeTunnelBase } from './api-client'
import { listSavedHosts, recordSavedHost } from './saved-hosts'
import { useHostStore } from '../state/host-store'

beforeEach(() => {
  localStorage.clear()
  useHostStore.setState({ currentHostId: null, label: null, platform: null, error: null, loading: false })
  vi.restoreAllMocks()
})
afterEach(() => {
  localStorage.clear()
  vi.restoreAllMocks()
})

describe('switchActiveHost', () => {
  it('returns session-expired on 401 without removing saved host', async () => {
    storeApiKey('host1', 'key')
    storeTunnelBase('host1', 'https://host1.tunnel.cf')
    recordSavedHost({ host_id: 'host1', label: 'Host 1', api_key_last4: 'key1' })

    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(null, { status: 401 }),
    )
    const navigate = vi.fn()
    const result = await switchActiveHost('host1', navigate)
    expect(result).toEqual({ ok: false, error: 'session-expired' })
    expect(listSavedHosts().find((h) => h.host_id === 'host1')).toBeDefined()
    expect(navigate).not.toHaveBeenCalled()
  })

  it('returns network on fetch rejection without removing saved host', async () => {
    recordSavedHost({ host_id: 'host1', label: 'Host 1', api_key_last4: 'key1' })
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('network down'))
    const navigate = vi.fn()
    const result = await switchActiveHost('host1', navigate)
    expect(result).toEqual({ ok: false, error: 'network' })
    expect(listSavedHosts().find((h) => h.host_id === 'host1')).toBeDefined()
    expect(navigate).not.toHaveBeenCalled()
  })

  it('navigates and resolves ok on successful host switch', async () => {
    storeApiKey('host1', 'key')
    storeTunnelBase('host1', 'https://host1.tunnel.cf')
    const fetchMock = vi.spyOn(globalThis, 'fetch')
    // /api/me probe → 200, then /api/host → matching host
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 200 }))
    fetchMock.mockResolvedValueOnce(
      new Response(
        JSON.stringify({ host_id: 'host1', label: 'Host 1', platform: 'linux' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    const navigate = vi.fn()
    const result = await switchActiveHost('host1', navigate)
    expect(result).toEqual({ ok: true })
    expect(navigate).toHaveBeenCalledWith('/h/host1/workspace', { replace: true })
    expect(useHostStore.getState().currentHostId).toBe('host1')
  })
})
