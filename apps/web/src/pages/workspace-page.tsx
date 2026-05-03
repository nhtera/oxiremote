import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import 'xterm/css/xterm.css'

import {
  useTerminalStore,
  type Session,
  type PaneIndex,
} from '../state/terminal-store'
import {
  loadPrefs,
  type TerminalPrefs,
} from '../lib/terminal-prefs'
import { MAX_RECONNECT_ATTEMPTS, backoffMsForAttempt } from '../lib/terminal-ws-hook'
import TerminalTabBar from '../components/terminal-tab-bar'
import TerminalKeybar from '../components/terminal-keybar'
import TerminalSendComposer from '../components/terminal-send-composer'
import TerminalSettingsPopover from '../components/terminal-settings-popover'
import ReconnectModal from '../components/reconnect-modal'
import NewSessionRow from '../components/workspace/new-session-row'
import MultiPaneGrid from '../components/workspace/multi-pane-grid'
import KeyboardShortcutOverlay from '../components/workspace/keyboard-shortcut-overlay'
import SessionStatusBar from '../components/workspace/session-status-bar'
import { useSessionConnectionState } from '../hooks/use-session-connection-state'
import { StateView, useConfirm } from '../components/ui'

type CreateSessionReq = { cols: number; rows: number; name?: string }
type CreateSessionRes = { id: string }


export default function WorkspacePage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { sessionId: sessionIdParam, hostId } = useParams<{ sessionId?: string; hostId?: string }>()
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
  const [shortcutOverlayOpen, setShortcutOverlayOpen] = useState(false)
  // Brief toast for clipboard-deny feedback from the mobile Paste button.
  const [keybarToast, setKeybarToast] = useState<string | null>(null)
  const {
    connectedById,
    reconnectAttemptById,
    reconnectExhaustedById,
    reconnectingById,
    handleConnectedChange,
    handleReconnectAttempt,
    handleReconnectExhausted,
    clearSession,
    resetReconnect,
  } = useSessionConnectionState()

  // Map<sessionId, sendFn> registered by each XtermPane while it's mounted.
  // The bottom composer dispatches to the focused pane via this map.
  const sendFnsRef = useRef<Map<string, (data: string) => void>>(new Map())
  // Companion map for selection getters — drives the mobile "Copy" affordance.
  const selectionFnsRef = useRef<Map<string, () => string>>(new Map())

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
  // If the host has no live sessions and we haven't already auto-created on
  // this tab, mint one so the user lands on a usable terminal instead of an
  // empty-state screen. The sessionStorage flag prevents a duplicate-create
  // race when StrictMode double-fires the effect or the page remounts.
  useEffect(() => {
    const remembered = sessionIdParam || searchParams.get('session') || localStorage.getItem('oxi:last-terminal-session')
    let cancelled = false
    void refreshSessions().then((data) => {
      if (cancelled || !data) return
      const live = data.filter((s) => s.state !== 'exited')
      const found = remembered ? data.find((s) => s.id === remembered) ?? null : null
      const bestId = (found?.state !== 'exited' ? found?.id : null)
        ?? live.find((s) => s.state === 'active' || s.state === 'idle')?.id
        ?? live[0]?.id
        ?? null
      if (bestId) {
        if (!paneAssignments.includes(bestId)) {
          attachToPane(0, bestId)
          localStorage.setItem('oxi:last-terminal-session', bestId)
        }
        return
      }
      if (live.length === 0 && !sessionStorage.getItem('oxi:skip-autocreate')) {
        sessionStorage.setItem('oxi:skip-autocreate', '1')
        // Debounce briefly so transient empty hydration doesn't double-create.
        window.setTimeout(() => {
          if (!cancelled) void createSession()
        }, 200)
      }
    })
    return () => { cancelled = true }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionIdParam])

  // Auto-clear keybar toast after 3 s.
  useEffect(() => {
    if (!keybarToast) return
    const id = window.setTimeout(() => setKeybarToast(null), 3000)
    return () => window.clearTimeout(id)
  }, [keybarToast])

  // ? keydown → open shortcut overlay (skip when xterm canvas has focus).
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== '?') return
      const active = document.activeElement
      // xterm mounts a <canvas> or <textarea> for its input; skip when focused.
      if (active && (active.tagName === 'CANVAS' || active.tagName === 'TEXTAREA' || active.tagName === 'INPUT')) return
      e.preventDefault()
      setShortcutOverlayOpen((v) => !v)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [])

  // beforeunload guard — warn on tab close / refresh when sessions are active.
  const hasActiveSessions = sessions.some(
    (s) => s.state !== 'exited' && connectedById[s.id],
  )
  useEffect(() => {
    function onBeforeUnload(e: BeforeUnloadEvent) {
      if (!hasActiveSessions) return
      e.preventDefault()
    }
    window.addEventListener('beforeunload', onBeforeUnload)
    return () => window.removeEventListener('beforeunload', onBeforeUnload)
  }, [hasActiveSessions])

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
    clearSession(id)

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

  const registerGetSelection = useMemo(
    () => (sessionId: string, fn: (() => string) | null) => {
      if (fn) selectionFnsRef.current.set(sessionId, fn)
      else selectionFnsRef.current.delete(sessionId)
    },
    [],
  )

  const getFocusedSelection = useCallback((): string => {
    if (!focusedSessionId) return ''
    return selectionFnsRef.current.get(focusedSessionId)?.() ?? ''
  }, [focusedSessionId])

  const focusedAttempt = focusedSessionId ? (reconnectAttemptById[focusedSessionId] ?? 0) : 0
  const focusedExhausted = focusedSessionId ? !!reconnectExhaustedById[focusedSessionId] : false

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
          hostId={hostId}
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
        <SessionStatusBar
          active={active}
          focusedSessionId={focusedSessionId}
          isFocusedConnected={isFocusedConnected}
          paneCount={paneCount}
          err={err}
          onReconnect={() => {
            if (!focusedSessionId) return
            resetReconnect(focusedSessionId)
            setReconnectNonce((n) => n + 1)
          }}
          onCloseFocused={() => {
            if (focusedSessionId) closeSession(focusedSessionId)
          }}
          onPaneCountChange={setPaneCount}
        />
      )}

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
          registerGetSelection={registerGetSelection}
        />
      ) : (
        <div className="flex-1 min-h-0 bg-dot-grid flex flex-col items-center justify-center gap-6 px-4">
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
          {keybarToast && (
            <div className="text-[length:var(--text-meta)] text-warning mb-1 px-1">{keybarToast}</div>
          )}
          <TerminalKeybar
            onSend={sendInput}
            onToast={(msg) => setKeybarToast(msg)}
            getSelection={getFocusedSelection}
          />
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
            resetReconnect(focusedSessionId)
            closeSession(focusedSessionId, { skipConfirm: true })
          } else {
            void refreshSessions()
          }
        }}
        onRetry={
          focusedExhausted
            ? undefined
            : () => {
                if (focusedSessionId) resetReconnect(focusedSessionId)
                setReconnectNonce((n) => n + 1)
              }
        }
      />

      <KeyboardShortcutOverlay
        open={shortcutOverlayOpen}
        onClose={() => setShortcutOverlayOpen(false)}
      />

    </div>
  )
}
