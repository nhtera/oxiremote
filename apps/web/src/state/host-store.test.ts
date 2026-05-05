import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useHostStore } from './host-store'

beforeEach(() => {
  localStorage.clear()
  useHostStore.setState({ currentHostId: null, label: null, platform: null, error: null, loading: false })
  vi.restoreAllMocks()
})
afterEach(() => {
  localStorage.clear()
  vi.restoreAllMocks()
})

describe('fetchHost', () => {
  it('sets store state from /api/host on success when no expectedHostId', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ host_id: 'host1', label: 'Host 1', platform: 'macOS' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    await useHostStore.getState().fetchHost()
    expect(useHostStore.getState().currentHostId).toBe('host1')
    expect(useHostStore.getState().error).toBeNull()
  })

  it('sets error on host_id mismatch when expectedHostId provided', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ host_id: 'mac', label: 'Mac', platform: 'macOS' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    await useHostStore.getState().fetchHost('windows')
    expect(useHostStore.getState().currentHostId).toBeNull()
    expect(useHostStore.getState().error).toMatch(/mismatch/i)
  })

  it('deduplicates concurrent calls into one HTTP request', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ host_id: 'host1', label: 'H', platform: 'Linux' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    await Promise.all([
      useHostStore.getState().fetchHost(),
      useHostStore.getState().fetchHost(),
      useHostStore.getState().fetchHost(),
    ])
    expect(fetchSpy).toHaveBeenCalledTimes(1)
  })

  it('treats 401 as a no-op without overwriting state', async () => {
    useHostStore.setState({ currentHostId: 'previous', label: 'Prev', platform: 'macOS' })
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(null, { status: 401 }),
    )
    await useHostStore.getState().fetchHost()
    // Existing state preserved; loading flag cleared.
    expect(useHostStore.getState().currentHostId).toBe('previous')
    expect(useHostStore.getState().loading).toBe(false)
  })
})
