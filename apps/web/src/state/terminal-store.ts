import { create } from 'zustand'

export type SessionState = 'idle' | 'active' | 'exited'

export type Session = {
  id: string
  name: string | null
  state: SessionState
  host_id: string
  created_at: number
  last_seen_at: number
  cols: number
  rows: number
  // legacy field from existing API; kept for compat
  status: string
  exit_code: number | null
}

type TerminalStoreState = {
  sessions: Session[]
  activeId: string | null
  setActive: (id: string | null) => void
  upsert: (session: Session) => void
  remove: (id: string) => void
  rename: (id: string, name: string) => void
  setState: (id: string, state: SessionState) => void
  setSessions: (sessions: Session[]) => void
}

export const useTerminalStore = create<TerminalStoreState>((set) => ({
  sessions: [],
  activeId: null,

  setActive: (id) => set({ activeId: id }),

  upsert: (session) =>
    set((s) => {
      const idx = s.sessions.findIndex((x) => x.id === session.id)
      if (idx >= 0) {
        const next = [...s.sessions]
        next[idx] = { ...next[idx], ...session }
        return { sessions: next }
      }
      return { sessions: [...s.sessions, session] }
    }),

  remove: (id) =>
    set((s) => ({
      sessions: s.sessions.filter((x) => x.id !== id),
      activeId: s.activeId === id ? null : s.activeId,
    })),

  rename: (id, name) =>
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, name } : x)),
    })),

  setState: (id, state) =>
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, state } : x)),
    })),

  setSessions: (sessions) => set({ sessions }),
}))
