import { create } from 'zustand'
import { isDiscoveryMode } from '../lib/discovery-client'
import { loadTunnelBase } from '../lib/api-client'

type HostState = {
  currentHostId: string | null
  label: string | null
  platform: string | null
  loading: boolean
  error: string | null
  fetchHost: () => Promise<void>
}

export const useHostStore = create<HostState>((set) => ({
  currentHostId: null,
  label: null,
  platform: null,
  loading: false,
  error: null,

  fetchHost: async () => {
    // In discovery mode without a paired host, /api/host has nowhere to go
    // (the SPA origin has no API). Skip the probe to avoid the router
    // redirect-loop via the SPA fallback. Once a pair completes the
    // interceptor rewrites /api/host to the tunnel base — this branch
    // unblocks itself naturally.
    if (isDiscoveryMode() && !loadTunnelBase()) {
      set({ loading: false })
      return
    }
    set({ loading: true, error: null })
    try {
      const res = await fetch('/api/host', { credentials: 'include' })
      if (res.status === 401) {
        // Not authenticated — router handles redirect; leave state untouched
        set({ loading: false })
        return
      }
      if (!res.ok) {
        set({ loading: false, error: `Failed to fetch host (${res.status})` })
        return
      }
      const data = (await res.json()) as { host_id: string; label: string; platform: string }
      set({
        loading: false,
        currentHostId: data.host_id,
        label: data.label,
        platform: data.platform,
      })
    } catch (e) {
      set({ loading: false, error: String(e) })
    }
  },
}))
