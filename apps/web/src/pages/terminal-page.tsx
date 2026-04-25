import { useEffect, useMemo, useRef, useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'

import { useTerminalStore, type Session } from '../state/terminal-store'
import { terminalThemes, loadTheme, saveTheme } from '../lib/terminal-themes'
import { useTerminalWs, destroyHandle, type SessionHandle } from '../lib/terminal-ws-hook'
import TerminalTabBar from '../components/terminal-tab-bar'
import TerminalKeybar from '../components/terminal-keybar'
import TerminalSendComposer from '../components/terminal-send-composer'

function debounce<F extends (...args: unknown[]) => void>(fn: F, ms: number) {
  let t: number | undefined
  return (...args: Parameters<F>) => {
    if (t) window.clearTimeout(t)
    t = window.setTimeout(() => fn(...args), ms)
  }
}

type CreateSessionReq = { cols: number; rows: number }
type CreateSessionRes = { id: string }

// Settings popover — minimal theme picker
function ThemeSettings({ onClose }: { onClose: () => void }) {
  const [current, setCurrent] = useState(loadTheme())
  return (
    <div className="absolute top-10 right-0 z-50 bg-surface-alt border border-border rounded-lg shadow-lg p-3 min-w-45">
      <div className="text-xs font-medium text-text-secondary mb-2">Theme</div>
      {Object.keys(terminalThemes).map((key) => (
        <button
          key={key}
          onClick={() => { saveTheme(key); setCurrent(key); onClose() }}
          className={`w-full text-left text-xs px-2 py-1.5 rounded transition-colors ${
            current === key ? 'bg-accent/15 text-accent' : 'text-text-secondary hover:bg-surface-hover'
          }`}
        >
          {key}
        </button>
      ))}
    </div>
  )
}

export default function TerminalPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { sessionId: sessionIdParam } = useParams<{ sessionId?: string }>()
  const { sessions, activeId, setActive, setSessions, rename, setState } = useTerminalStore()
  const [err, setErr] = useState<string | null>(null)
  const [isConnected, setIsConnected] = useState(false)
  const [reconnectNonce, setReconnectNonce] = useState(0)
  const [showSettings, setShowSettings] = useState(false)
  const [themeKey, setThemeKey] = useState(loadTheme)

  const containerRef = useRef<HTMLDivElement | null>(null)
  const handlesRef = useRef<Map<string, SessionHandle>>(new Map())
  const activeIdRef = useRef<string | null>(null)

  const active = useMemo(() => sessions.find((s) => s.id === activeId) ?? null, [sessions, activeId])

  useEffect(() => {
    activeIdRef.current = activeId
    if (activeId) {
      const q = new URLSearchParams(searchParams)
      q.set('session', activeId)
      setSearchParams(q, { replace: true })
    }
  }, [activeId, searchParams, setSearchParams])

  function getOrCreateHandle(id: string): SessionHandle {
    const existing = handlesRef.current.get(id)
    if (existing) return existing
    const theme = terminalThemes[themeKey] ?? terminalThemes.default
    const term = new Terminal({
      cursorBlink: true, fontSize: 14,
      fontFamily: 'ui-monospace, Consolas, monospace',
      theme,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    const handle: SessionHandle = {
      term,
      fit,
      ws: null,
      connected: false,
      reconnectTimer: null,
      reconnectAttempt: 0,
      closedByUser: false,
    }
    handlesRef.current.set(id, handle)
    return handle
  }

  const { refreshSessions } = useTerminalWs(handlesRef, {
    activeId,
    reconnectNonce,
    onConnected: setIsConnected,
    onError: setErr,
  })

  // Apply theme change to existing terminals
  useEffect(() => {
    const theme = terminalThemes[themeKey] ?? terminalThemes.default
    for (const handle of handlesRef.current.values()) {
      handle.term.options.theme = theme
    }
  }, [themeKey])

  useEffect(() => {
    const el = containerRef.current
    if (!el || !activeId) return
    const handle = getOrCreateHandle(activeId)
    el.replaceChildren()
    handle.term.open(el)
    handle.fit.fit()
    handle.term.focus()
    setIsConnected(handle.connected)
    // The WS hook's effect runs before this one, so on first mount for a
    // session it sees no handle and bails. Bump nonce so it re-runs now that
    // the handle exists. No-op for already-connected sockets (connect() is
    // idempotent against handle.ws state).
    setReconnectNonce((n) => n + 1)

    const debouncedResize = debounce(() => {
      handle.fit.fit()
      if (activeIdRef.current === activeId) {
        fetch(`/api/terminal/sessions/${activeId}/resize`, {
          method: 'POST', credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ cols: handle.term.cols, rows: handle.term.rows }),
        })
      }
    }, 200)
    const ro = new ResizeObserver(() => debouncedResize())
    ro.observe(el)
    return () => { ro.disconnect() }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId])

  useEffect(() => {
    // Precedence: path param (:sessionId, from deep links) > ?session= query > last-used localStorage.
    const remembered = sessionIdParam || searchParams.get('session') || localStorage.getItem('oxi:last-terminal-session')
    refreshSessions().then((data?: Session[]) => {
      if (!data) return
      const found = remembered ? data.find((s) => s.id === remembered) ?? null : null
      const bestId = (found?.state !== 'exited' ? found?.id : null)
        ?? data.find((s) => s.state === 'active' || s.state === 'idle')?.id
        ?? data[0]?.id ?? null
      if (bestId) { setActive(bestId); localStorage.setItem('oxi:last-terminal-session', bestId) }
    })
    return () => { for (const id of handlesRef.current.keys()) destroyHandle(handlesRef, id) }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionIdParam])

  async function createSession() {
    setErr(null)
    const h = activeId ? handlesRef.current.get(activeId) : null
    const body: CreateSessionReq = { cols: h?.term.cols ?? 80, rows: h?.term.rows ?? 24 }
    const res = await fetch('/api/terminal/sessions', {
      method: 'POST', credentials: 'include',
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    })
    if (!res.ok) { setErr(await res.text()); return }
    const data = (await res.json()) as CreateSessionRes
    await refreshSessions()
    setActive(data.id)
    localStorage.setItem('oxi:last-terminal-session', data.id)
  }

  async function closeSession(id: string) {
    const res = await fetch(`/api/terminal/sessions/${id}/close`, { method: 'POST', credentials: 'include' })
    if (!res.ok) { setErr('Failed to close session'); return }
    destroyHandle(handlesRef, id)
    await refreshSessions()
  }

  async function handleRename(id: string, name: string) {
    rename(id, name) // optimistic
    const res = await fetch(`/api/terminal/sessions/${id}`, {
      method: 'PATCH', credentials: 'include',
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name }),
    })
    if (!res.ok) {
      // Revert on failure by refreshing
      await refreshSessions()
    }
  }

  function sendInput(data: string) {
    if (!activeId) return
    const handle = handlesRef.current.get(activeId)
    if (handle?.ws?.readyState === WebSocket.OPEN) {
      handle.ws.send(JSON.stringify({ t: 'input', data }))
      handle.term.focus()
    }
  }

  function handleSettingsClose() {
    setShowSettings(false)
    setThemeKey(loadTheme()) // pick up saved theme
  }

  // Keep setState/rename in scope for the WS hook (accessed via store directly)
  void setState; void rename; void setSessions

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Tab bar */}
      <div className="relative">
        <TerminalTabBar
          sessions={sessions}
          activeId={activeId}
          onSelect={(id) => { setActive(id); localStorage.setItem('oxi:last-terminal-session', id) }}
          onClose={closeSession}
          onNew={createSession}
          onRename={handleRename}
          onOpenSettings={() => setShowSettings((v) => !v)}
        />
        {showSettings && <ThemeSettings onClose={handleSettingsClose} />}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-2 px-3 py-1 shrink-0 border-b border-border bg-surface/95 backdrop-blur">
        <span className={`text-[11px] px-2 py-0.5 rounded-full border ${isConnected ? 'text-success border-success/30 bg-success/10' : 'text-warning border-warning/30 bg-warning/10'}`}>
          {activeId ? (isConnected ? 'Connected' : 'Disconnected') : 'No session'}
        </span>
        {!isConnected && activeId && (
          <button onClick={() => setReconnectNonce((n) => n + 1)} className="btn-secondary text-xs py-0.5 px-2 text-warning">
            Reconnect
          </button>
        )}
        {active && (
          <span className="text-xs text-text-muted ml-auto">
            {active.id.slice(0, 8)} · {active.cols}×{active.rows}
          </span>
        )}
        {err && <span className="text-danger text-xs ml-auto truncate max-w-50">{err}</span>}
      </div>

      {/* Terminal canvas */}
      <div
        ref={containerRef}
        onClick={() => handlesRef.current.get(activeId ?? '')?.term.focus()}
        className="flex-1 min-h-0 overflow-hidden"
      />

      {/* Virtual keybar — mobile only; desktop hides it (Phase 02 adds opt-in toggle) */}
      <div className="md:hidden px-2 py-1.5 border-t border-border bg-surface-alt shrink-0">
        <TerminalKeybar onSend={sendInput} />
      </div>

      {/* Mobile send composer */}
      <TerminalSendComposer onSend={sendInput} />
    </div>
  )
}
