import { useEffect, useRef } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import { useTerminalStore, type Session } from '../state/terminal-store'

export type SessionHandle = {
  term: Terminal
  fit: FitAddon
  ws: WebSocket | null
  connected: boolean
}

function wsUrl(path: string) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

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
  const { setState, rename, setSessions } = useTerminalStore.getState()

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

    if (!handle.ws || handle.ws.readyState > WebSocket.OPEN) {
      if (handle.ws) { handle.ws.close(); handle.ws = null }

      const ws = new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`))
      handle.ws = ws

      ws.onopen = () => {
        handle.connected = true
        if (activeIdRef.current === sessionId) onConnected(true)
      }
      ws.onclose = () => {
        handle.connected = false
        if (activeIdRef.current === sessionId) {
          onConnected(false)
          handle.term.writeln('\r\n\x1b[33m[disconnected]\x1b[0m')
        }
      }
      ws.onmessage = (ev) => {
        try {
          // Tolerant: ignore unknown message types
          const msg = JSON.parse(ev.data) as Record<string, unknown>
          if (msg.t === 'output' && typeof msg.data === 'string') {
            handle.term.write(msg.data)
          } else if (msg.t === 'exit') {
            handle.term.writeln('\r\n\x1b[90m[process exited]\x1b[0m')
            refreshSessions()
          } else if (msg.t === 'state' && typeof msg.state === 'string') {
            setState(sessionId, msg.state as Session['state'])
          } else if (msg.t === 'renamed' && typeof msg.name === 'string') {
            rename(sessionId, msg.name)
          }
          // Unknown variants silently ignored per spec
        } catch {}
      }
    } else {
      onConnected(handle.connected)
    }

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

export function destroyHandle(
  handlesRef: React.MutableRefObject<Map<string, SessionHandle>>,
  id: string
) {
  const handle = handlesRef.current.get(id)
  if (!handle) return
  handle.ws?.close()
  handle.term.dispose()
  handlesRef.current.delete(id)
}
