import { useEffect, useMemo, useRef, useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import 'xterm/css/xterm.css'

import {
  useTerminalStore,
  type Session,
  type PaneCount,
  type PaneIndex,
} from '../state/terminal-store'
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
import { MAX_RECONNECT_ATTEMPTS, backoffMsForAttempt } from '../lib/terminal-ws-hook'
import TerminalTabBar from '../components/terminal-tab-bar'
import TerminalKeybar from '../components/terminal-keybar'
import TerminalSendComposer from '../components/terminal-send-composer'
import ReconnectModal from '../components/reconnect-modal'
import NewSessionRow from '../components/workspace/new-session-row'
import MultiPaneGrid from '../components/workspace/multi-pane-grid'
import { StateView, useConfirm } from '../components/ui'

type CreateSessionReq = { cols: number; rows: number; name?: string }
type CreateSessionRes = { id: string }

interface SettingsProps {
  prefs: TerminalPrefs
  onChange: (next: TerminalPrefs) => void
  onClose: () => void
}

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

function SplitToggle({ value, onChange }: { value: PaneCount; onChange: (n: PaneCount) => void }) {
  return (
    <div className="hidden md:inline-flex rounded-md border border-border overflow-hidden text-[11px]" role="tablist" aria-label="Pane split">
      {([1, 2, 3] as PaneCount[]).map((n) => (
        <button
          key={n}
          onClick={() => onChange(n)}
          className={`px-2 py-0.5 transition-colors ${
            value === n
              ? 'bg-accent/15 text-accent'
              : 'text-text-muted hover:bg-surface-hover hover:text-text-primary'
          }`}
          role="tab"
          aria-selected={value === n}
          title={`${n}-pane split`}
        >
          {n}-up
        </button>
      ))}
    </div>
  )
}

export default function WorkspacePage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { sessionId: sessionIdParam } = useParams<{ sessionId?: string }>()
  const {
    sessions, activeId, setActive, setSessions, rename, remove,
    paneAssignments, paneCount, focusedPane,
    attachToPane, setPaneCount, setFocusedPane,
  } = useTerminalStore()
  const confirm = useConfirm()
  const [err, setErr] = useState<string | null>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [prefs, setPrefs] = useState<TerminalPrefs>(loadPrefs)
  const [reconnectNonce, setReconnectNonce] = useState(0)
  // Per-session connection telemetry. Drives tab-bar dots, the focused pane's
  // status pill, and the reconnect modal. Records (not zustand) keep this
  // ephemeral state out of the global store.
  const [connectedById, setConnectedById] = useState<Record<string, boolean>>({})
  const [reconnectAttemptById, setReconnectAttemptById] = useState<Record<string, number>>({})
  const [reconnectExhaustedById, setReconnectExhaustedById] = useState<Record<string, boolean>>({})

  // Map<sessionId, sendFn> registered by each XtermPane while it's mounted.
  // The bottom composer dispatches to the focused pane via this map.
  const sendFnsRef = useRef<Map<string, (data: string) => void>>(new Map())

  // Tombstones — older agent builds left exited rows in list responses.
  const closedIdsRef = useRef<Set<string>>(new Set())

  const focusedSessionId = paneAssignments[focusedPane] ?? null
  const active = useMemo(
    () => sessions.find((s) => s.id === focusedSessionId) ?? null,
    [sessions, focusedSessionId],
  )
  const isFocusedConnected = focusedSessionId ? !!connectedById[focusedSessionId] : false

  useEffect(() => {
    if (focusedSessionId && focusedSessionId !== activeId) setActive(focusedSessionId)
  }, [focusedSessionId, activeId, setActive])

  useEffect(() => {
    if (activeId) {
      const q = new URLSearchParams(searchParams)
      q.set('session', activeId)
      setSearchParams(q, { replace: true })
    }
  }, [activeId, searchParams, setSearchParams])

  async function refreshSessions(): Promise<Session[] | undefined> {
    const res = await fetch('/api/terminal/sessions', { credentials: 'include' })
    if (!res.ok) {
      setErr('Not authorized. Pair first at /login.')
      return undefined
    }
    const data = (await res.json()) as Session[]
    const tombstones = closedIdsRef.current
    const filtered = tombstones.size > 0 ? data.filter((s) => !tombstones.has(s.id)) : data
    setSessions(filtered)
    return filtered
  }

  // Initial load — fetch sessions, attach the best one to pane 0.
  useEffect(() => {
    const remembered = sessionIdParam || searchParams.get('session') || localStorage.getItem('oxi:last-terminal-session')
    void refreshSessions().then((data) => {
      if (!data) return
      const found = remembered ? data.find((s) => s.id === remembered) ?? null : null
      const bestId = (found?.state !== 'exited' ? found?.id : null)
        ?? data.find((s) => s.state === 'active' || s.state === 'idle')?.id
        ?? data[0]?.id
        ?? null
      if (bestId && !paneAssignments.includes(bestId)) {
        attachToPane(0, bestId)
        localStorage.setItem('oxi:last-terminal-session', bestId)
      }
    })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionIdParam])

  function defaultName(): string {
    const used = new Set<number>()
    for (const s of sessions) {
      const m = s.name?.match(/^Terminal (\d+)$/)
      if (m) used.add(Number(m[1]))
    }
    let n = 1
    while (used.has(n)) n++
    return `Terminal ${n}`
  }

  async function createSession(nameOverride?: string) {
    setErr(null)
    const body: CreateSessionReq = {
      cols: 80, rows: 24,
      name: (nameOverride && nameOverride.length > 0) ? nameOverride : defaultName(),
    }
    const res = await fetch('/api/terminal/sessions', {
      method: 'POST', credentials: 'include',
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    })
    if (!res.ok) { setErr(await res.text()); return }
    const data = (await res.json()) as CreateSessionRes
    await refreshSessions()
    // Land it in the focused pane so the new tab is immediately interactive.
    attachToPane(focusedPane, data.id)
    localStorage.setItem('oxi:last-terminal-session', data.id)
  }

  async function closeSession(id: string, opts: { skipConfirm?: boolean } = {}) {
    if (!opts.skipConfirm) {
      const target = sessions.find((s) => s.id === id)
      const label = target?.name ?? `session ${id.slice(0, 8)}`
      const ok = await confirm({
        title: 'Close session',
        message: `Close "${label}"? The shell process will be terminated and any unsaved work in it will be lost.`,
        confirmText: 'Close', cancelText: 'Cancel', danger: true,
      })
      if (!ok) return
    }
    closedIdsRef.current.add(id)
    // store.remove() also clears any pane that held this session.
    remove(id)
    setConnectedById((m) => { const { [id]: _, ...rest } = m; return rest })
    setReconnectAttemptById((m) => { const { [id]: _, ...rest } = m; return rest })
    setReconnectExhaustedById((m) => { const { [id]: _, ...rest } = m; return rest })

    const res = await fetch(`/api/terminal/sessions/${id}/close`, { method: 'POST', credentials: 'include' })
    if (!res.ok && res.status !== 410) setErr('Failed to close session')
    await refreshSessions()
  }

  async function handleRename(id: string, name: string) {
    rename(id, name)
    const res = await fetch(`/api/terminal/sessions/${id}`, {
      method: 'PATCH', credentials: 'include',
      headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name }),
    })
    if (!res.ok) await refreshSessions()
  }

  function selectSession(id: string) {
    // Clicking a tab assigns the session into the focused pane (or moves it
    // there if it was already in another pane). Matches the spec: "Click tab
    // → assigns to focused pane".
    attachToPane(focusedPane, id)
    localStorage.setItem('oxi:last-terminal-session', id)
  }

  function sendInput(data: string) {
    if (!focusedSessionId) return
    const send = sendFnsRef.current.get(focusedSessionId)
    send?.(data)
  }

  const registerSend = useMemo(
    () => (sessionId: string, fn: ((data: string) => void) | null) => {
      if (fn) sendFnsRef.current.set(sessionId, fn)
      else sendFnsRef.current.delete(sessionId)
    },
    [],
  )

  const handleConnectedChange = (sessionId: string, connected: boolean) => {
    setConnectedById((m) => (m[sessionId] === connected ? m : { ...m, [sessionId]: connected }))
    if (connected) {
      setReconnectAttemptById((m) => ({ ...m, [sessionId]: 0 }))
      setReconnectExhaustedById((m) => ({ ...m, [sessionId]: false }))
    }
  }
  const handleReconnectAttempt = (sessionId: string, attempt: number) =>
    setReconnectAttemptById((m) => ({ ...m, [sessionId]: attempt }))
  const handleReconnectExhausted = (sessionId: string) =>
    setReconnectExhaustedById((m) => ({ ...m, [sessionId]: true }))

  const focusedAttempt = focusedSessionId ? (reconnectAttemptById[focusedSessionId] ?? 0) : 0
  const focusedExhausted = focusedSessionId ? !!reconnectExhaustedById[focusedSessionId] : false
  // A session is "reconnecting" if its WS-hook reported an attempt and it
  // hasn't reconnected yet. Drives the orange tab dot for non-focused panes
  // too — useful when a pane on the side drops while you're typing in another.
  const reconnectingById = useMemo(() => {
    const m: Record<string, boolean> = {}
    for (const [id, n] of Object.entries(reconnectAttemptById)) {
      if (n > 0 && !connectedById[id]) m[id] = true
    }
    return m
  }, [reconnectAttemptById, connectedById])

  const hasSessions = sessions.length > 0
  const anyAssigned = paneAssignments.some((s) => s !== null)

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="relative">
        <TerminalTabBar
          sessions={sessions}
          activeId={focusedSessionId}
          isActiveConnected={isFocusedConnected}
          connectedById={connectedById}
          reconnectingById={reconnectingById}
          onSelect={selectSession}
          onClose={closeSession}
          onNew={() => void createSession()}
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

      {hasSessions && (() => {
        const isExited = active?.state === 'exited'
        const pillClass = isExited
          ? 'text-danger border-danger/30 bg-danger/10'
          : isFocusedConnected
            ? 'text-success border-success/30 bg-success/10'
            : 'text-warning border-warning/30 bg-warning/10'
        const pillLabel = !focusedSessionId
          ? 'No session'
          : isExited ? 'Exited' : isFocusedConnected ? 'Connected' : 'Disconnected'
        return (
          <div className="flex items-center gap-2 px-3 py-1 shrink-0 border-b border-border bg-surface/95 backdrop-blur">
            <span className={`text-[11px] px-2 py-0.5 rounded-full border ${pillClass}`}>
              {pillLabel}
            </span>
            {!isFocusedConnected && focusedSessionId && !isExited && (
              <button
                onClick={() => {
                  setReconnectExhaustedById((m) => ({ ...m, [focusedSessionId]: false }))
                  setReconnectAttemptById((m) => ({ ...m, [focusedSessionId]: 0 }))
                  setReconnectNonce((n) => n + 1)
                }}
                className="btn-secondary text-xs py-0.5 px-2 text-warning"
              >
                Reconnect
              </button>
            )}
            {isExited && focusedSessionId && (
              <button
                onClick={() => closeSession(focusedSessionId)}
                className="btn-secondary text-xs py-0.5 px-2"
                title="The PTY has exited — close this tab"
              >
                Close tab
              </button>
            )}
            <SplitToggle value={paneCount} onChange={setPaneCount} />
            {active && (
              <span className="text-xs text-text-muted ml-auto">
                {(active.name ?? active.id.slice(0, 8))} · {active.cols}×{active.rows}
              </span>
            )}
            {err && <span className="text-danger text-xs ml-auto truncate max-w-50">{err}</span>}
          </div>
        )
      })()}

      {hasSessions && anyAssigned ? (
        <MultiPaneGrid
          paneCount={paneCount}
          paneAssignments={paneAssignments}
          focusedPane={focusedPane}
          prefs={prefs}
          reconnectNonce={reconnectNonce}
          onFocusPane={(idx: PaneIndex) => setFocusedPane(idx)}
          onConnectedChange={handleConnectedChange}
          onReconnectAttempt={handleReconnectAttempt}
          onReconnectExhausted={handleReconnectExhausted}
          onError={setErr}
          registerSend={registerSend}
        />
      ) : (
        <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-6 px-4">
          <StateView
            icon={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <polyline points="4 6 10 12 4 18" />
                <line x1="12" y1="18" x2="20" y2="18" />
              </svg>
            }
            title={hasSessions ? 'No pane assigned' : 'No active sessions'}
            body={hasSessions
              ? 'Pick a tab above to load it into a pane, or create a new session.'
              : 'Create a new session to get started. Each session keeps a persistent PTY so you can detach and resume later.'}
          />
          <NewSessionRow onCreate={createSession} />
          {err && <div className="text-danger text-xs">{err}</div>}
        </div>
      )}

      {hasSessions && anyAssigned && (
        <div className="md:hidden px-2 py-1.5 border-t border-border bg-surface-alt shrink-0">
          <TerminalKeybar onSend={sendInput} />
        </div>
      )}

      {hasSessions && anyAssigned && <TerminalSendComposer onSend={sendInput} />}

      <ReconnectModal
        open={!!focusedSessionId && active?.state !== 'exited' && !isFocusedConnected && (focusedAttempt > 0 || focusedExhausted)}
        attempt={focusedAttempt}
        maxAttempts={MAX_RECONNECT_ATTEMPTS}
        exhausted={focusedExhausted}
        countdownMs={backoffMsForAttempt(focusedAttempt)}
        onCancel={() => {
          if (focusedSessionId) {
            setReconnectExhaustedById((m) => ({ ...m, [focusedSessionId]: false }))
            setReconnectAttemptById((m) => ({ ...m, [focusedSessionId]: 0 }))
            closeSession(focusedSessionId, { skipConfirm: true })
          } else {
            void refreshSessions()
          }
        }}
        onRetry={
          focusedExhausted
            ? undefined
            : () => {
                if (focusedSessionId) {
                  setReconnectExhaustedById((m) => ({ ...m, [focusedSessionId]: false }))
                  setReconnectAttemptById((m) => ({ ...m, [focusedSessionId]: 0 }))
                }
                setReconnectNonce((n) => n + 1)
              }
        }
      />
    </div>
  )
}
