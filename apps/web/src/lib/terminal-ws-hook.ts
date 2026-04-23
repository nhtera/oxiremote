import { useEffect, useRef } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import { useTerminalStore, type Session } from '../state/terminal-store'

export type SessionHandle = {
  term: Terminal
  fit: FitAddon
  ws: WebSocket | null
  connected: boolean
  // Reconnect bookkeeping — kept on the handle so backoff survives re-render.
  reconnectTimer: number | null
  reconnectAttempt: number
  closedByUser: boolean
}

function wsUrl(path: string) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

// Exponential-ish backoff capped at 5s, matches "reconnect within 3s" success criterion.
const BACKOFF_MS = [500, 1000, 2000, 3000, 5000]

type Options = {
  activeId: string | null
  reconnectNonce: number
  onConnected: (connected: boolean) => void
  onError: (msg: string) => void
}

export function useTerminalWs(
  handlesRef: React.MutableRefObject<Map<string, SessionHandle>>,
  options: Options
) {
  const { activeId, reconnectNonce, onConnected, onError } = options
  const activeIdRef = useRef<string | null>(null)
  const { setSessions } = useTerminalStore.getState()

  useEffect(() => {
    activeIdRef.current = activeId
  }, [activeId])

  async function refreshSessions() {
    const res = await fetch('/api/terminal/sessions', { credentials: 'include' })
    if (!res.ok) {
      onError('Not authorized. Pair first at /login.')
      return
    }
    const data = (await res.json()) as Session[]
    setSessions(data)

    const ids = new Set(data.map((s) => s.id))
    for (const id of handlesRef.current.keys()) {
      if (!ids.has(id)) destroyHandle(handlesRef, id)
    }
    return data
  }

  useEffect(() => {
    if (!activeId) return
    const handle = handlesRef.current.get(activeId)
    if (!handle) return

    const sessionId = activeId
    handle.closedByUser = false
    connect(handle, sessionId, onConnected, activeIdRef)

    const onData = handle.term.onData((data) => {
      if (handle.ws?.readyState === WebSocket.OPEN) {
        handle.ws.send(JSON.stringify({ t: 'input', data }))
      }
    })

    return () => { onData.dispose() }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId, reconnectNonce])

  return { refreshSessions }
}

function connect(
  handle: SessionHandle,
  sessionId: string,
  onConnected: (c: boolean) => void,
  activeIdRef: React.MutableRefObject<string | null>
) {
  if (handle.reconnectTimer != null) {
    window.clearTimeout(handle.reconnectTimer)
    handle.reconnectTimer = null
  }

  if (handle.ws && handle.ws.readyState < WebSocket.CLOSING) {
    // Already connecting or open — nothing to do.
    return
  }

  const ws = new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`))
  handle.ws = ws

  ws.onopen = () => {
    handle.connected = true
    handle.reconnectAttempt = 0
    if (activeIdRef.current === sessionId) onConnected(true)
    // Send attach first — server will reply with snapshot, then live chunks.
    const lastSeq = useTerminalStore.getState().lastSeqById[sessionId]
    ws.send(JSON.stringify({ t: 'attach', last_seq: lastSeq ?? null }))
  }

  ws.onclose = () => {
    handle.connected = false
    handle.ws = null
    if (activeIdRef.current === sessionId) onConnected(false)
    if (handle.closedByUser) return
    // Auto-reconnect with backoff.
    const delay = BACKOFF_MS[Math.min(handle.reconnectAttempt, BACKOFF_MS.length - 1)]
    handle.reconnectAttempt += 1
    handle.reconnectTimer = window.setTimeout(() => {
      handle.reconnectTimer = null
      connect(handle, sessionId, onConnected, activeIdRef)
    }, delay)
  }

  ws.onmessage = (ev) => {
    try {
      // Tolerant: ignore unknown message types
      const msg = JSON.parse(ev.data) as Record<string, unknown>
      const { setLastSeq, setState, rename } = useTerminalStore.getState()
      if (msg.t === 'chunk' && typeof msg.data === 'string' && typeof msg.seq === 'number') {
        handle.term.write(msg.data)
        setLastSeq(sessionId, msg.seq)
      } else if (msg.t === 'snapshot' && typeof msg.data === 'string' && typeof msg.to_seq === 'number') {
        if (msg.data.length > 0) handle.term.write(msg.data)
        if (msg.to_seq > 0) setLastSeq(sessionId, msg.to_seq)
      } else if (msg.t === 'exit') {
        handle.term.writeln('\r\n\x1b[90m[process exited]\x1b[0m')
        handle.closedByUser = true // don't reconnect to a dead session
      } else if (msg.t === 'state' && typeof msg.state === 'string') {
        setState(sessionId, msg.state as Session['state'])
      } else if (msg.t === 'renamed' && typeof msg.name === 'string') {
        rename(sessionId, msg.name)
      }
      // Unknown variants silently ignored per spec
    } catch {}
  }
}

export function destroyHandle(
  handlesRef: React.MutableRefObject<Map<string, SessionHandle>>,
  id: string
) {
  const handle = handlesRef.current.get(id)
  if (!handle) return
  handle.closedByUser = true
  if (handle.reconnectTimer != null) {
    window.clearTimeout(handle.reconnectTimer)
    handle.reconnectTimer = null
  }
  handle.ws?.close()
  handle.term.dispose()
  useTerminalStore.getState().resetLastSeq(id)
  handlesRef.current.delete(id)
}
