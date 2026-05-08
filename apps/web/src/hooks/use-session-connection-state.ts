import { useMemo, useState } from 'react'
import type { ReconnectPhase } from '../lib/terminal-ws-hook'

// Per-session telemetry for the workspace page. Tracks connected/disconnected
// flags, reconnect attempt counts, and the active retry phase ('fast' for
// transient drops, 'slow' once the host appears genuinely offline). Drives
// tab-bar dots, the focused pane's status pill, and the reconnect modal copy.
// Records (not zustand) keep this ephemeral state out of the global store.
export function useSessionConnectionState() {
  const [connectedById, setConnectedById] = useState<Record<string, boolean>>({})
  const [reconnectAttemptById, setReconnectAttemptById] = useState<Record<string, number>>({})
  const [phaseById, setPhaseById] = useState<Record<string, ReconnectPhase>>({})

  const handleConnectedChange = (sessionId: string, connected: boolean) => {
    setConnectedById((m) => (m[sessionId] === connected ? m : { ...m, [sessionId]: connected }))
    if (connected) {
      setReconnectAttemptById((m) => ({ ...m, [sessionId]: 0 }))
      setPhaseById((m) => (m[sessionId] === 'fast' ? m : { ...m, [sessionId]: 'fast' }))
    }
  }
  const handleReconnectAttempt = (sessionId: string, attempt: number) =>
    setReconnectAttemptById((m) => ({ ...m, [sessionId]: attempt }))
  const handleReconnectPhase = (sessionId: string, phase: ReconnectPhase) =>
    setPhaseById((m) => (m[sessionId] === phase ? m : { ...m, [sessionId]: phase }))

  // A session is "reconnecting" if its WS-hook reported an attempt and it
  // hasn't reconnected yet. Drives the orange tab dot for non-focused panes
  // too — useful when a pane on the side drops while you're typing elsewhere.
  const reconnectingById = useMemo(() => {
    const m: Record<string, boolean> = {}
    for (const [id, n] of Object.entries(reconnectAttemptById)) {
      if (n > 0 && !connectedById[id]) m[id] = true
    }
    return m
  }, [reconnectAttemptById, connectedById])

  function clearSession(id: string) {
    const drop = <T,>(m: Record<string, T>): Record<string, T> => {
      if (!(id in m)) return m
      const { [id]: _drop, ...rest } = m
      void _drop
      return rest
    }
    setConnectedById(drop)
    setReconnectAttemptById(drop)
    setPhaseById(drop)
  }

  function resetReconnect(id: string) {
    setReconnectAttemptById((m) => ({ ...m, [id]: 0 }))
    setPhaseById((m) => ({ ...m, [id]: 'fast' }))
  }

  return {
    connectedById,
    reconnectAttemptById,
    phaseById,
    reconnectingById,
    handleConnectedChange,
    handleReconnectAttempt,
    handleReconnectPhase,
    clearSession,
    resetReconnect,
  }
}
