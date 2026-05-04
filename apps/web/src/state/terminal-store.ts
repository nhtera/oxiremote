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
  last_seq?: number
  buffer_bytes?: number
  attached?: boolean
  // Agent-aware moat fields (nullable — old servers omit them)
  detected_agent?: string | null
  notify_on_agent_end?: boolean
}

// Workspace can host up to 3 horizontal panes. Each pane is either empty or
// bound to a sessionId. The focused pane receives keyboard input from the
// composer/keybar and is the click target for tab-bar selection.
export type PaneIndex = 0 | 1 | 2
export type PaneCount = 1 | 2 | 3
export type PaneAssignments = [string | null, string | null, string | null]

type TerminalStoreState = {
  sessions: Session[]
  activeId: string | null
  // Client-side ack of latest applied seq per session. Used on reconnect.
  lastSeqById: Record<string, number>
  paneAssignments: PaneAssignments
  paneCount: PaneCount
  focusedPane: PaneIndex
  setActive: (id: string | null) => void
  upsert: (session: Session) => void
  remove: (id: string) => void
  rename: (id: string, name: string) => void
  setState: (id: string, state: SessionState) => void
  setSessions: (sessions: Session[]) => void
  setLastSeq: (id: string, seq: number) => void
  resetLastSeq: (id: string) => void
  attachToPane: (paneIdx: PaneIndex, sessionId: string) => void
  detachFromPane: (paneIdx: PaneIndex) => void
  setPaneCount: (n: PaneCount) => void
  setFocusedPane: (idx: PaneIndex) => void
  /** Set or clear the detected agent name for a session. */
  setDetectedAgent: (id: string, agentName: string | null) => void
  /** Update the notify-on-finish opt-in flag after a successful API call. */
  setNotifyOnAgentEnd: (id: string, enabled: boolean) => void
}

export const useTerminalStore = create<TerminalStoreState>((set) => ({
  sessions: [],
  activeId: null,
  lastSeqById: {},
  paneAssignments: [null, null, null],
  paneCount: 1,
  focusedPane: 0,

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
      // Drop the closed session from any pane that held it; otherwise the
      // pane keeps a stale id and the slot renders an empty placeholder.
      const nextAssignments = s.paneAssignments.map((a) => (a === id ? null : a)) as PaneAssignments
      return {
        sessions: s.sessions.filter((x) => x.id !== id),
        activeId: s.activeId === id ? null : s.activeId,
        lastSeqById: rest,
        paneAssignments: nextAssignments,
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

  attachToPane: (paneIdx, sessionId) =>
    set((s) => {
      // If the session is already in another pane, move it (don't duplicate —
      // a session can't render into two panes since xterm owns the DOM node).
      const next = s.paneAssignments.map((a) => (a === sessionId ? null : a)) as PaneAssignments
      next[paneIdx] = sessionId
      return { paneAssignments: next, focusedPane: paneIdx, activeId: sessionId }
    }),

  detachFromPane: (paneIdx) =>
    set((s) => {
      const next = [...s.paneAssignments] as PaneAssignments
      next[paneIdx] = null
      return { paneAssignments: next }
    }),

  setPaneCount: (n) =>
    set((s) => {
      // Shrinking the grid drops assignments past the new last index so
      // those panes don't keep WS connections open invisibly.
      const arr: (string | null)[] = [...s.paneAssignments]
      for (let i = n; i < 3; i++) arr[i] = null
      const next = arr as unknown as PaneAssignments
      const focused = s.focusedPane >= n ? ((n - 1) as PaneIndex) : s.focusedPane
      return { paneCount: n, paneAssignments: next, focusedPane: focused }
    }),

  setFocusedPane: (idx) =>
    set((s) => {
      const sid = s.paneAssignments[idx]
      return { focusedPane: idx, activeId: sid ?? s.activeId }
    }),

  setDetectedAgent: (id, agentName) =>
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, detected_agent: agentName } : x,
      ),
    })),

  setNotifyOnAgentEnd: (id, enabled) =>
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, notify_on_agent_end: enabled } : x,
      ),
    })),
}))
