import { create } from 'zustand'

export interface Workspace {
  id: number
  path: string
  label: string
  last_used_at: number
  pinned: boolean
}

interface ActiveEntry {
  id: number
  path: string
  label: string
}

type WorkspaceState = {
  active: Record<string, ActiveEntry>
  list: Workspace[]
  loading: boolean
  error: string | null
  setActive: (hostId: string, ws: ActiveEntry) => void
  clearActive: (hostId: string) => void
  fetchList: () => Promise<void>
  createWorkspace: (path: string, label?: string) => Promise<{ ok: true } | { ok: false; error: string }>
  deleteWorkspace: (id: number) => Promise<void>
  touch: (id: number) => void
}

const STORAGE_KEY = 'oxi.activeWs'

function loadPersisted(): Record<string, ActiveEntry> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw)
    return typeof parsed === 'object' && parsed ? parsed : {}
  } catch {
    return {}
  }
}

function persist(active: Record<string, ActiveEntry>) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(active))
  } catch {
    /* quota / private mode */
  }
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  active: loadPersisted(),
  list: [],
  loading: false,
  error: null,

  setActive: (hostId, ws) => {
    const next = { ...get().active, [hostId]: ws }
    persist(next)
    set({ active: next })
  },

  clearActive: (hostId) => {
    const next = { ...get().active }
    delete next[hostId]
    persist(next)
    set({ active: next })
  },

  fetchList: async () => {
    set({ loading: true, error: null })
    try {
      const res = await fetch('/api/workspaces', { credentials: 'include' })
      if (!res.ok) {
        set({ loading: false, error: `Failed (${res.status})` })
        return
      }
      const data = (await res.json()) as Workspace[]
      set({ list: data, loading: false })
    } catch (e) {
      set({ loading: false, error: String(e) })
    }
  },

  createWorkspace: async (path, label) => {
    const res = await fetch('/api/workspaces', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path, label }),
    })
    if (!res.ok) {
      return { ok: false, error: (await res.text()) || `error ${res.status}` }
    }
    const data = (await res.json()) as Workspace[]
    set({ list: data })
    return { ok: true }
  },

  deleteWorkspace: async (id) => {
    const res = await fetch(`/api/workspaces/${id}`, {
      method: 'DELETE',
      credentials: 'include',
    })
    if (res.ok) {
      set({ list: get().list.filter((w) => w.id !== id) })
    }
  },

  touch: (id) => {
    void fetch(`/api/workspaces/${id}/touch`, {
      method: 'POST',
      credentials: 'include',
    })
  },
}))
