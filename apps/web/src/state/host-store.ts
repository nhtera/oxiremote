import { create } from 'zustand'

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
