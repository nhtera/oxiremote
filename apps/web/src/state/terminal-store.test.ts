import { describe, it, expect, beforeEach } from 'vitest'
import { useTerminalStore, type Session } from './terminal-store'

function makeSession(id: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    name: null,
    state: 'active',
    host_id: 'host-1',
    created_at: 0,
    last_seen_at: 0,
    cols: 80,
    rows: 24,
    status: 'active',
    exit_code: null,
    ...overrides,
  }
}

beforeEach(() => {
  useTerminalStore.setState({
    sessions: [],
    activeId: null,
    lastSeqById: {},
    paneAssignments: [null, null, null],
    paneCount: 1,
    focusedPane: 0,
  })
})

describe('terminal-store pane assignment', () => {
  it('attachToPane places session and focuses pane', () => {
    useTerminalStore.getState().attachToPane(1, 's1')
    const s = useTerminalStore.getState()
    expect(s.paneAssignments).toEqual([null, 's1', null])
    expect(s.focusedPane).toBe(1)
    expect(s.activeId).toBe('s1')
  })

  it('attachToPane moves session if already in another pane (no duplicates)', () => {
    const api = useTerminalStore.getState()
    api.attachToPane(0, 's1')
    api.attachToPane(2, 's1')
    expect(useTerminalStore.getState().paneAssignments).toEqual([null, null, 's1'])
  })

  it('detachFromPane clears the slot', () => {
    const api = useTerminalStore.getState()
    api.attachToPane(0, 's1')
    api.detachFromPane(0)
    expect(useTerminalStore.getState().paneAssignments[0]).toBeNull()
  })

  it('setPaneCount drops assignments past the new last index', () => {
    const api = useTerminalStore.getState()
    api.attachToPane(0, 's1')
    api.attachToPane(1, 's2')
    api.attachToPane(2, 's3')
    api.setPaneCount(2)
    const s = useTerminalStore.getState()
    expect(s.paneAssignments).toEqual(['s1', 's2', null])
    expect(s.paneCount).toBe(2)
  })

  it('setPaneCount clamps focusedPane when shrinking', () => {
    const api = useTerminalStore.getState()
    api.attachToPane(2, 's3')
    expect(useTerminalStore.getState().focusedPane).toBe(2)
    api.setPaneCount(1)
    expect(useTerminalStore.getState().focusedPane).toBe(0)
  })

  it('remove(id) clears the session from any pane it occupied', () => {
    const api = useTerminalStore.getState()
    api.upsert(makeSession('s1'))
    api.attachToPane(1, 's1')
    api.remove('s1')
    const s = useTerminalStore.getState()
    expect(s.paneAssignments).toEqual([null, null, null])
    expect(s.activeId).toBeNull()
  })

  it('setFocusedPane updates activeId from that pane', () => {
    const api = useTerminalStore.getState()
    api.attachToPane(0, 's1')
    api.attachToPane(2, 's2')
    api.setFocusedPane(0)
    expect(useTerminalStore.getState().activeId).toBe('s1')
  })
})
