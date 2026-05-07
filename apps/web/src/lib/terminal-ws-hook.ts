import { useEffect, useRef } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import { useTerminalStore, type Session } from '../state/terminal-store'
import { isDiscoveryMode } from './discovery-client'
import { loadApiKey, loadTunnelBase } from './api-client'
import { mintWsTicket, WS_TICKET_PROTOCOL } from './ws-ticket-client'

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

/// Legacy subprotocol marker — carried the api_key on WS upgrade in
/// discovery mode. Phase 05 / H14 replaced it with `oxi-ticket-v1`; this
/// fallback stays for one release so an old SPA tab doesn't break the
/// moment the agent updates. Removed in v0.1.27.
const WS_BEARER_PROTOCOL = 'oxi-bearer-v1'

function wsUrl(path: string): string {
  if (isDiscoveryMode()) {
    const base = loadTunnelBase()
    if (base) {
      const wsBase = base.replace(/^http:/, 'ws:').replace(/^https:/, 'wss:')
      return `${wsBase}${path}`
    }
  }
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

/** Resolve subprotocols for a fresh WS open. Tries to mint a single-use
 *  ticket first (Phase 05 / H14); falls back to the legacy bearer flow when
 *  the agent doesn't yet expose `/api/ws-ticket` (older release) or the
 *  request fails for transient reasons. Same-origin embedded mode skips
 *  this entirely — cookie auth is enough. */
async function wsProtocols(): Promise<string[] | undefined> {
  const ticket = await mintWsTicket()
  if (ticket) return [WS_TICKET_PROTOCOL, ticket]
  if (!isDiscoveryMode()) return undefined
  const key = loadApiKey()
  if (!key) return undefined
  return [WS_BEARER_PROTOCOL, key]
}

// Exponential-ish backoff capped at 5s, matches "reconnect within 3s" success criterion.
export const BACKOFF_MS = [500, 1000, 2000, 3000, 5000]

/** Backoff (in ms) the hook will wait before attempt N (1-indexed). UI uses
 *  this to keep the reconnect modal's countdown / progress bar in sync. */
export function backoffMsForAttempt(attempt: number): number {
  const idx = Math.max(0, Math.min(attempt - 1, BACKOFF_MS.length - 1))
  return BACKOFF_MS[idx]
}
// Stop trying after 8 attempts (~21s of cumulative back-off + the WS handshake
// time-out). Past this the disconnect is almost always permanent — agent
// crash, device revoke, network split — and silently retrying forever both
// burns battery and hides the failure from the user. Surface it via the
// reconnect modal so they can choose to retry or give up.
export const MAX_RECONNECT_ATTEMPTS = 8

type Options = {
  activeId: string | null
  reconnectNonce: number
  onConnected: (connected: boolean) => void
  onError: (msg: string) => void
  /** Fired once when the reconnect cap is hit; UI uses this to surface the modal. */
  onReconnectExhausted?: () => void
  /** Fired on each new reconnect attempt so the UI can show "attempt N of M". */
  onReconnectAttempt?: (attempt: number) => void
}

export function useTerminalWs(
  handlesRef: React.MutableRefObject<Map<string, SessionHandle>>,
  options: Options
) {
  const { activeId, reconnectNonce, onConnected, onError, onReconnectExhausted, onReconnectAttempt } = options
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
    // If the session is already exited, don't open a WS — the server will
    // immediately close it (no PTY) and we'd loop on reconnect, then surface
    // the "Connection lost" modal over a tab the user already knows is dead.
    const sess = useTerminalStore.getState().sessions.find((s) => s.id === sessionId)
    if (sess?.state === 'exited') {
      handle.closedByUser = true
      handle.connected = false
      onConnected(false)
      return
    }
    handle.closedByUser = false
    handle.reconnectAttempt = 0
    connect(handle, sessionId, onConnected, activeIdRef, onReconnectExhausted, onReconnectAttempt)

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
  activeIdRef: React.MutableRefObject<string | null>,
  onReconnectExhausted?: () => void,
  onReconnectAttempt?: (attempt: number) => void,
) {
  if (handle.reconnectTimer != null) {
    window.clearTimeout(handle.reconnectTimer)
    handle.reconnectTimer = null
  }

  if (handle.ws && handle.ws.readyState < WebSocket.CLOSING) {
    // Already connecting or open — nothing to do.
    return
  }

  // Mint the ticket BEFORE opening the WS so the upgrade handshake carries
  // the freshest 60s token. The mint is async (HTTP round-trip); guard
  // against teardown happening between mint and open — `closedByUser` is
  // set by `destroyHandle` and `useTerminalWs`'s exit-state branch, so
  // checking it here covers the unmount-during-mint race.
  void wsProtocols().then((protocols) => {
    if (handle.closedByUser) return
    if (handle.ws && handle.ws.readyState < WebSocket.CLOSING) return
    const ws = protocols
      ? new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`), protocols)
      : new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`))
    handle.ws = ws
    wireWs(ws, handle, sessionId, onConnected, activeIdRef, onReconnectExhausted, onReconnectAttempt)
  })
}

function wireWs(
  ws: WebSocket,
  handle: SessionHandle,
  sessionId: string,
  onConnected: (c: boolean) => void,
  activeIdRef: React.MutableRefObject<string | null>,
  onReconnectExhausted?: () => void,
  onReconnectAttempt?: (attempt: number) => void,
) {

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
    // Cap reconnect attempts so a permanent failure (agent crash, device
    // revoke) surfaces in the UI instead of looping in the background
    // forever and burning battery on a connection that will never come back.
    if (handle.reconnectAttempt >= MAX_RECONNECT_ATTEMPTS) {
      handle.closedByUser = true
      onReconnectExhausted?.()
      return
    }
    // Auto-reconnect with backoff.
    const delay = BACKOFF_MS[Math.min(handle.reconnectAttempt, BACKOFF_MS.length - 1)]
    handle.reconnectAttempt += 1
    onReconnectAttempt?.(handle.reconnectAttempt)
    handle.reconnectTimer = window.setTimeout(() => {
      handle.reconnectTimer = null
      connect(handle, sessionId, onConnected, activeIdRef, onReconnectExhausted, onReconnectAttempt)
    }, delay)
  }

  ws.onmessage = (ev) => {
    try {
      // Tolerant: ignore unknown message types
      const msg = JSON.parse(ev.data) as Record<string, unknown>
      const { setLastSeq, setState, rename, setDetectedAgent } = useTerminalStore.getState()
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
      } else if (msg.t === 'agent_detected' && typeof msg.agent_name === 'string') {
        // Agent CLI appeared in PTY foreground — show badge in tab bar.
        setDetectedAgent(sessionId, msg.agent_name)
      } else if (msg.t === 'agent_ended') {
        // Agent CLI exited — clear badge.
        setDetectedAgent(sessionId, null)
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
