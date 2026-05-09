import { useEffect, useRef } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import { useTerminalStore, type Session } from '../state/terminal-store'
import { isDiscoveryMode, getCurrentTunnelUrl } from './discovery-client'
import { getActiveHost, loadApiKey, loadTunnelBase, storeTunnelBase } from './api-client'
import { probeHost, refreshTunnelBaseFromDiscovery } from './host-reachability'
import { isAllowedTunnelHost, getNamedTunnelAllowlist } from './url-validation'
import { TUNNEL_URL_CHANGED_EVENT } from '../hooks/use-tunnel-url-sse'

export type ReconnectPhase = 'fast' | 'slow'

export type SessionHandle = {
  term: Terminal
  fit: FitAddon
  ws: WebSocket | null
  connected: boolean
  // Reconnect bookkeeping — kept on the handle so backoff survives re-render.
  reconnectTimer: number | null
  reconnectAttempt: number
  closedByUser: boolean
  // Two-phase backoff. 'fast' uses BACKOFF_MS for transient drops; 'slow'
  // uses SLOW_BACKOFF_MS once /api/health probing confirms the host is down.
  phase: ReconnectPhase
  slowAttempt: number
  // Rate-limit /api/health probes (one per ~3s).
  lastProbeAt: number
  // Bumped on manual retry / mount so an in-flight async probe knows to bail
  // if the user has triggered a fresh reconnect path in the meantime.
  scheduleEpoch: number
}

/// Subprotocol marker used to carry a Bearer api_key on WS upgrade in
/// discovery (cross-origin) mode. Browsers can't set `Authorization` on
/// the WS handshake, but they can offer subprotocols. The agent picks the
/// marker (response header) and reads the api_key from the second value.
/// Same pattern Kubernetes uses for `kubectl exec`.
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

function wsProtocols(): string[] | undefined {
  if (!isDiscoveryMode()) return undefined
  const key = loadApiKey()
  if (!key) return undefined
  return [WS_BEARER_PROTOCOL, key]
}

// Fast ladder for transient drops (handshake race, brief tunnel hiccup).
// Cumulative ~12s — covers reconnect storms without making the user wait too
// long before we surface a clearer "host appears offline" message.
export const BACKOFF_MS = [500, 1000, 2000, 3000, 5000]

// Slow ladder once /api/health says the host is unreachable (sleep/wake,
// Wi-Fi handoff, agent restart). Cycles at 30s; we never give up
// automatically — the modal lets the user explicitly bail out via "Exit",
// and connectivity events (online / visibilitychange / focus) wake the loop
// up immediately when the device comes back.
export const SLOW_BACKOFF_MS = [5000, 10000, 15000, 30000]

/** Backoff (in ms) the hook will wait before fast-phase attempt N
 *  (1-indexed). UI uses this to keep the reconnect modal's countdown / progress
 *  bar in sync. Slow phase doesn't surface a per-attempt countdown. */
export function backoffMsForAttempt(attempt: number): number {
  const idx = Math.max(0, Math.min(attempt - 1, BACKOFF_MS.length - 1))
  return BACKOFF_MS[idx]
}

// Exposed for the reconnect modal's progress bar denominator while in fast
// phase. Once the hook transitions to slow phase the modal hides the bar
// and switches its copy from "Reconnecting…" to "Host appears offline".
export const MAX_RECONNECT_ATTEMPTS = BACKOFF_MS.length

const PROBE_RATE_LIMIT_MS = 3000
// Tiny delay between a "host is alive" probe and the next WS attempt —
// avoids a fast spin if probeHost returns alive but the WS handshake
// immediately fails again (e.g. agent is up but route layer is still warming).
const FAST_RECOVERY_DELAY_MS = 250

type Options = {
  activeId: string | null
  reconnectNonce: number
  onConnected: (connected: boolean) => void
  onError: (msg: string) => void
  /** Fired on each new reconnect attempt so the UI can show "attempt N of M". */
  onReconnectAttempt?: (attempt: number) => void
  /** Fired when the hook switches between fast (transient drop) and slow
   *  (host appears offline) reconnect phases. Drives the modal's copy. */
  onReconnectPhase?: (phase: ReconnectPhase) => void
}

export function useTerminalWs(
  handlesRef: React.MutableRefObject<Map<string, SessionHandle>>,
  options: Options
) {
  const { activeId, reconnectNonce, onConnected, onError, onReconnectAttempt, onReconnectPhase } = options
  const activeIdRef = useRef<string | null>(null)
  const { setSessions } = useTerminalStore.getState()

  useEffect(() => {
    activeIdRef.current = activeId
  }, [activeId])

  // When the supervisor rotates the tunnel URL, close all active WS connections
  // so their onclose handlers schedule a reconnect via the updated tunnel base.
  useEffect(() => {
    function onTunnelUrlChanged() {
      for (const handle of handlesRef.current.values()) {
        if (handle.ws && handle.ws.readyState < WebSocket.CLOSING) {
          // Tear down cleanly; onclose fires and schedules reconnect.
          handle.ws.close()
        }
      }
    }
    window.addEventListener(TUNNEL_URL_CHANGED_EVENT, onTunnelUrlChanged)
    return () => window.removeEventListener(TUNNEL_URL_CHANGED_EVENT, onTunnelUrlChanged)
  // handlesRef is stable (same ref object across renders) — no dep needed.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

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
    handle.slowAttempt = 0
    handle.phase = 'fast'
    // Invalidate any in-flight async scheduleReconnect so a stale probe
    // resolution doesn't queue a duplicate WS attempt after this fresh path.
    handle.scheduleEpoch += 1
    onReconnectPhase?.('fast')
    connect(handle, sessionId, onConnected, activeIdRef, onReconnectAttempt, onReconnectPhase)

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
  onReconnectAttempt?: (attempt: number) => void,
  onReconnectPhase?: (phase: ReconnectPhase) => void,
) {
  if (handle.reconnectTimer != null) {
    window.clearTimeout(handle.reconnectTimer)
    handle.reconnectTimer = null
  }

  if (handle.ws && handle.ws.readyState < WebSocket.CLOSING) {
    // Already connecting or open — nothing to do.
    return
  }

  const protocols = wsProtocols()
  const ws = protocols
    ? new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`), protocols)
    : new WebSocket(wsUrl(`/api/terminal/sessions/${sessionId}/ws`))
  handle.ws = ws

  ws.onopen = () => {
    handle.connected = true
    handle.reconnectAttempt = 0
    handle.slowAttempt = 0
    if (handle.phase !== 'fast') {
      handle.phase = 'fast'
      onReconnectPhase?.('fast')
    }
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
    void scheduleReconnect(handle, sessionId, onConnected, activeIdRef, onReconnectAttempt, onReconnectPhase)
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

async function scheduleReconnect(
  handle: SessionHandle,
  sessionId: string,
  onConnected: (c: boolean) => void,
  activeIdRef: React.MutableRefObject<string | null>,
  onReconnectAttempt?: (attempt: number) => void,
  onReconnectPhase?: (phase: ReconnectPhase) => void,
) {
  if (handle.closedByUser) return

  // On the very first reconnect attempt, try resolving the current tunnel URL
  // via the discovery worker. This handles the cold-load stale-URL case (agent
  // restarted with a new Quick Tunnel before the SPA tab was refreshed). The
  // SSE path handles mid-session URL rotation; this covers the gap before SSE
  // connects. Concurrent calls from desktop hooks share the coalesced promise.
  if (handle.reconnectAttempt === 0) {
    try {
      const fresh = await getCurrentTunnelUrl()
      const cached = loadTunnelBase()
      if (fresh && fresh !== cached) {
        const allowlist = getNamedTunnelAllowlist()
        if (isAllowedTunnelHost(fresh, allowlist)) {
          const hostId = getActiveHost()
          if (hostId) storeTunnelBase(hostId, fresh)
        }
      }
    } catch {
      // Discovery not configured or worker unreachable — proceed with cached URL.
    }
  }

  // Fast phase: walk the existing ladder. Covers brief drops where probing
  // would just add latency before the next WS attempt.
  if (handle.phase === 'fast' && handle.reconnectAttempt < BACKOFF_MS.length) {
    const delay = BACKOFF_MS[handle.reconnectAttempt]
    handle.reconnectAttempt += 1
    onReconnectAttempt?.(handle.reconnectAttempt)
    handle.reconnectTimer = window.setTimeout(() => {
      handle.reconnectTimer = null
      connect(handle, sessionId, onConnected, activeIdRef, onReconnectAttempt, onReconnectPhase)
    }, delay)
    return
  }

  // Past the fast ladder (or already in slow phase). Probe /api/health
  // (rate-limited) to decide between staying in slow and jumping back to
  // fast — the latter handles "host went away briefly and is back now",
  // e.g. MacBook woke from sleep.
  const epoch = handle.scheduleEpoch
  let probeAlive = false
  const now = Date.now()
  if (now - handle.lastProbeAt >= PROBE_RATE_LIMIT_MS) {
    handle.lastProbeAt = now
    const id = getActiveHost()
    if (id) {
      // probeHost internally tries the cached tunnel base first, then
      // re-resolves via the discovery worker if that fails (covers Quick
      // Tunnel URL rotation after host sleep/wake). On a successful refresh
      // the new base is persisted, so the WS-URL builder picks it up on
      // the next connect() call.
      const result = await probeHost(id)
      probeAlive = result === 'alive'
      if (!probeAlive) {
        // Even when the probe stays unreachable, try refreshing the base
        // once: the agent may have just re-registered with the worker but
        // /api/health hasn't propagated yet, or the cached base is wrong
        // and the probe was hitting nothing. Worst case: no-op.
        await refreshTunnelBaseFromDiscovery(id)
      }
    }
  }

  // The probe await may have raced with a manual retry / mount that bumped
  // scheduleEpoch. If so the new code path now owns the schedule — bail to
  // avoid a duplicate WS connection.
  if (handle.scheduleEpoch !== epoch || handle.closedByUser) return

  if (probeAlive) {
    // Host is back. Reset to fast phase and retry on a tight delay; if the
    // WS still fails we'll walk the fast ladder again.
    handle.phase = 'fast'
    handle.reconnectAttempt = 1
    handle.slowAttempt = 0
    onReconnectPhase?.('fast')
    onReconnectAttempt?.(1)
    handle.reconnectTimer = window.setTimeout(() => {
      handle.reconnectTimer = null
      connect(handle, sessionId, onConnected, activeIdRef, onReconnectAttempt, onReconnectPhase)
    }, FAST_RECOVERY_DELAY_MS)
    return
  }

  // Unreachable / unknown / probe was rate-limited → slow phase.
  if (handle.phase !== 'slow') {
    handle.phase = 'slow'
    onReconnectPhase?.('slow')
  }
  const delay = SLOW_BACKOFF_MS[Math.min(handle.slowAttempt, SLOW_BACKOFF_MS.length - 1)]
  handle.slowAttempt += 1
  onReconnectAttempt?.(BACKOFF_MS.length + handle.slowAttempt)
  handle.reconnectTimer = window.setTimeout(() => {
    handle.reconnectTimer = null
    connect(handle, sessionId, onConnected, activeIdRef, onReconnectAttempt, onReconnectPhase)
  }, delay)
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
