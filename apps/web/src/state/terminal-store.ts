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
  // Phase 02: persistent PTY metadata
  last_seq?: number
  buffer_bytes?: number
  attached?: boolean
}

type TerminalStoreState = {
  sessions: Session[]
  activeId: string | null
  // Client-side ack of latest applied seq per session. Used on reconnect.
  lastSeqById: Record<string, number>
  setActive: (id: string | null) => void
  upsert: (session: Session) => void
  remove: (id: string) => void
  rename: (id: string, name: string) => void
  setState: (id: string, state: SessionState) => void
  setSessions: (sessions: Session[]) => void
  setLastSeq: (id: string, seq: number) => void
  resetLastSeq: (id: string) => void
}

export const useTerminalStore = create<TerminalStoreState>((set) => ({
  sessions: [],
  activeId: null,
  lastSeqById: {},

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
    set((s) => {
      const rest = { ...s.lastSeqById }
      delete rest[id]
      return {
        sessions: s.sessions.filter((x) => x.id !== id),
        activeId: s.activeId === id ? null : s.activeId,
        lastSeqById: rest,
      }
    }),

  rename: (id, name) =>
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, name } : x)),
    })),

  setState: (id, state) =>
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, state } : x)),
    })),

  setSessions: (sessions) => set({ sessions }),

  setLastSeq: (id, seq) =>
    set((s) => {
      const prev = s.lastSeqById[id] ?? 0
      if (seq <= prev) return s
      return { lastSeqById: { ...s.lastSeqById, [id]: seq } }
    }),

  resetLastSeq: (id) =>
    set((s) => {
      const rest = { ...s.lastSeqById }
      delete rest[id]
      return { lastSeqById: rest }
    }),
}))
