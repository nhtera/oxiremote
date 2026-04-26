import { useEffect, useMemo, useRef, useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'

import { useTerminalStore, type Session } from '../state/terminal-store'
import { terminalThemes } from '../lib/terminal-themes'
import {
  loadPrefs,
  savePrefs,
  CURSOR_OPTIONS,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  SCROLLBACK_OPTIONS,
  type TerminalPrefs,
} from '../lib/terminal-prefs'
import { useTerminalWs, destroyHandle, type SessionHandle } from '../lib/terminal-ws-hook'
import TerminalTabBar from '../components/terminal-tab-bar'
import TerminalKeybar from '../components/terminal-keybar'
import TerminalSendComposer from '../components/terminal-send-composer'
import { Button, StateView } from '../components/ui'

function debounce<F extends (...args: unknown[]) => void>(fn: F, ms: number) {
  let t: number | undefined
  return (...args: Parameters<F>) => {
    if (t) window.clearTimeout(t)
    t = window.setTimeout(() => fn(...args), ms)
  }
}

type CreateSessionReq = { cols: number; rows: number; name?: string }
type CreateSessionRes = { id: string }

interface SettingsProps {
  prefs: TerminalPrefs
  onChange: (next: TerminalPrefs) => void
  onClose: () => void
}

// Settings popover — theme, font size, scrollback, cursor style. Mirrors
// the breadth of 9Remote's terminal preferences while keeping the layout
// compact enough to fit on mobile (max-w bound, scrollable if cramped).
function TerminalSettingsPopover({ prefs, onChange, onClose }: SettingsProps) {
  const update = (patch: Partial<TerminalPrefs>) => {
    const merged = savePrefs(patch)
    onChange(merged)
  }
  return (
    <div className="absolute top-10 right-0 z-50 bg-surface-alt border border-border rounded-lg shadow-lg p-3 w-64 max-w-[calc(100vw-2rem)] space-y-3">
      <Section label="Theme">
        <div className="space-y-1">
          {Object.keys(terminalThemes).map((key) => (
            <button
              key={key}
              onClick={() => update({ theme: key })}
              className={`w-full text-left text-xs px-2 py-1.5 rounded transition-colors ${
                prefs.theme === key ? 'bg-accent/15 text-accent' : 'text-text-secondary hover:bg-surface-hover'
              }`}
            >
              {key}
            </button>
          ))}
        </div>
      </Section>
      <Section label="Font size">
        <div className="flex items-center gap-2">
          <input
            type="range"
            min={FONT_SIZE_MIN}
            max={FONT_SIZE_MAX}
            value={prefs.fontSize}
            onChange={(e) => update({ fontSize: Number(e.target.value) })}
            className="flex-1"
            aria-label="Font size"
          />
          <span className="text-xs text-text-muted w-8 text-right">{prefs.fontSize}px</span>
        </div>
      </Section>
      <Section label="Scrollback">
        <select
          value={prefs.scrollback}
          onChange={(e) => update({ scrollback: Number(e.target.value) })}
          className="w-full text-xs bg-surface border border-border rounded px-2 py-1 outline-none focus:border-accent/60"
        >
          {SCROLLBACK_OPTIONS.map((n) => (
            <option key={n} value={n}>{n.toLocaleString()} lines</option>
          ))}
        </select>
        <div className="text-[10px] text-text-muted mt-1">Applies to new sessions.</div>
      </Section>
      <Section label="Cursor">
        <div className="flex gap-1">
          {CURSOR_OPTIONS.map((opt) => (
            <button
              key={opt}
              onClick={() => update({ cursorStyle: opt })}
              className={`flex-1 text-xs px-2 py-1 rounded border transition-colors ${
                prefs.cursorStyle === opt
                  ? 'bg-accent/15 text-accent border-accent/40'
                  : 'bg-surface text-text-secondary border-border hover:bg-surface-hover'
              }`}
            >
              {opt}
            </button>
          ))}
        </div>
      </Section>
      <button
        onClick={onClose}
        className="w-full text-xs px-2 py-1 rounded bg-surface text-text-muted hover:bg-surface-hover hover:text-text-primary border border-border transition-colors"
      >
        Done
      </button>
    </div>
  )
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="text-[10px] font-medium uppercase tracking-wide text-text-muted mb-1.5">
        {label}
      </div>
      {children}
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
  const [prefs, setPrefs] = useState<TerminalPrefs>(loadPrefs)

  const containerRef = useRef<HTMLDivElement | null>(null)
  const handlesRef = useRef<Map<string, SessionHandle>>(new Map())
  const activeIdRef = useRef<string | null>(null)
  const prefsRef = useRef(prefs)
  useEffect(() => {
    prefsRef.current = prefs
  }, [prefs])

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
    const p = prefsRef.current
    const theme = terminalThemes[p.theme] ?? terminalThemes.default
    const term = new Terminal({
      cursorBlink: true,
      fontSize: p.fontSize,
      fontFamily: 'ui-monospace, Consolas, monospace',
      theme,
      scrollback: p.scrollback,
      cursorStyle: p.cursorStyle,
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

  // Apply prefs to all live terminals when the user changes them.
  // `scrollback` only takes effect for new sessions (xterm reads it once);
  // theme / font / cursor reflow live.
  useEffect(() => {
    const theme = terminalThemes[prefs.theme] ?? terminalThemes.default
    for (const handle of handlesRef.current.values()) {
      handle.term.options.theme = theme
      handle.term.options.fontSize = prefs.fontSize
      handle.term.options.cursorStyle = prefs.cursorStyle
      try { handle.fit.fit() } catch {
        // Container detached; will refit on next ResizeObserver tick.
      }
    }
  }, [prefs.theme, prefs.fontSize, prefs.cursorStyle])

  useEffect(() => {
    const el = containerRef.current
    if (!el || !activeId) return
    const handle = getOrCreateHandle(activeId)
    el.replaceChildren()
    handle.term.open(el)
    handle.fit.fit()
    handle.term.focus()
    setIsConnected(handle.connected)
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
    // Friendly default name so the tab bar reads "Terminal 1, Terminal 2…"
    // instead of opaque UUID slices. The user can still rename via dblclick.
    const nextN = sessions.length + 1
    const body: CreateSessionReq = {
      cols: h?.term.cols ?? 80,
      rows: h?.term.rows ?? 24,
      name: `Terminal ${nextN}`,
    }
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
    rename(id, name)
    const res = await fetch(`/api/terminal/sessions/${id}`, {
      method: 'PATCH', credentials: 'include',
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name }),
    })
    if (!res.ok) {
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

  void setState; void rename; void setSessions

  const hasSessions = sessions.length > 0

  return (
    <div className="flex flex-col h-full min-h-0">
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
        {showSettings && (
          <TerminalSettingsPopover
            prefs={prefs}
            onChange={setPrefs}
            onClose={() => setShowSettings(false)}
          />
        )}
      </div>

      {hasSessions && (
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
              {(active.name ?? active.id.slice(0, 8))} · {active.cols}×{active.rows}
            </span>
          )}
          {err && <span className="text-danger text-xs ml-auto truncate max-w-50">{err}</span>}
        </div>
      )}

      {hasSessions ? (
        <div
          ref={containerRef}
          onClick={() => handlesRef.current.get(activeId ?? '')?.term.focus()}
          className="flex-1 min-h-0 overflow-hidden"
        />
      ) : (
        <div className="flex-1 min-h-0 flex items-center justify-center">
          <StateView
            icon={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <polyline points="4 6 10 12 4 18" />
                <line x1="12" y1="18" x2="20" y2="18" />
              </svg>
            }
            title="No active sessions"
            body="Spin up a shell to get started. Each session keeps a persistent PTY so you can detach and resume later."
            action={
              <Button variant="primary" onClick={createSession}>+ New session</Button>
            }
          />
        </div>
      )}

      {hasSessions && (
        <div className="md:hidden px-2 py-1.5 border-t border-border bg-surface-alt shrink-0">
          <TerminalKeybar onSend={sendInput} />
        </div>
      )}

      {hasSessions && <TerminalSendComposer onSend={sendInput} />}
    </div>
  )
}
