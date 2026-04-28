import { create } from 'zustand'
import { isDiscoveryMode } from '../lib/discovery-client'

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
    // In discovery mode the SPA origin (Pages) has no /api/host. The agent
    // lives on the tunnel and Phase 4.5 will wire fetchHost to the saved
    // tunnel base. Until then, no-op pre-pair so we don't trigger router
    // redirect-loops via the 200-with-HTML SPA fallback.
    if (isDiscoveryMode()) {
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
