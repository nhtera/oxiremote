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
import { useConfirm } from '../components/ui'
import { useWorkspaceStore } from '../state/workspace-store'
import { useHostStore } from '../state/host-store'
import { uploadFile, type UploadError } from '../hooks/use-file-upload'
import { shellQuote } from '../lib/shell-quote'
import FileAttachSheet from '../components/file-attach-sheet'
import { ATTACHMENTS_DIR, ensureAttachmentsDir } from '../lib/ensure-attachments-dir'

type UploadEntry = {
  id: string
  fileName: string
  file: File
  paneIdx: PaneIndex
  pct: number
  state: 'uploading' | 'error'
  error?: UploadError | null
  abort: AbortController
}

type PreviewEntry = {
  id: string
  fileName: string
  file: File
  paneIdx: PaneIndex
}

const PREVIEW_AUTO_DISMISS_MS = 8000

type CreateSessionReq = { cols: number; rows: number; name?: string }
type CreateSessionRes = { id: string }


export default function WorkspacePage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { sessionId: sessionIdParam, hostId } = useParams<{ sessionId?: string; hostId?: string }>()
  const {
    sessions, activeId, setActive, setSessions, rename, remove,
    paneAssignments, paneCount, focusedPane,
    attachToPane, setPaneCount, setFocusedPane,
    setNotifyOnAgentEnd,
  } = useTerminalStore()
  const confirm = useConfirm()
  const [err, setErr] = useState<string | null>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [prefs, setPrefs] = useState<TerminalPrefs>(loadPrefs)
  const [reconnectNonce, setReconnectNonce] = useState(0)
  const [shortcutOverlayOpen, setShortcutOverlayOpen] = useState(false)
  // Brief toast for clipboard-deny feedback from the mobile Paste button.
  const [keybarToast, setKeybarToast] = useState<string | null>(null)
  // Desktop attach modal (mobile uses the composer's bottom-sheet variant).
  const [attachModalOpen, setAttachModalOpen] = useState(false)
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

  // Active workspace id is needed for paste/drop uploads. Mirrors the lookup
  // in TerminalSendComposer so paste/drop and the picker share one source.
  const currentHostId = useHostStore((s) => s.currentHostId)
  const activeWsMap = useWorkspaceStore((s) => s.active)
  const activeWsId = currentHostId ? activeWsMap[currentHostId]?.id : undefined

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

  // Per-pane upload + preview chip stacks. Paste/drop/desktop-attach push a
  // chip onto the focused (or drop-target) pane while the XHR streams; on
  // success the chip is replaced with a preview chip that auto-expires.
  const [uploads, setUploads] = useState<UploadEntry[]>([])
  const [previews, setPreviews] = useState<PreviewEntry[]>([])
  // Mirror of `uploads` for retryUpload — keeps the callback identity stable
  // across XHR progress events. Without this, deps include `uploads` and
  // every progress tick rebuilds the callback → re-renders all chips.
  const uploadsRef = useRef<UploadEntry[]>([])
  useEffect(() => { uploadsRef.current = uploads }, [uploads])
  // Track preview auto-dismiss timers so unmount can clear them and avoid
  // setState-on-unmounted warnings.
  const previewTimersRef = useRef<Set<number>>(new Set())
  useEffect(() => () => {
    previewTimersRef.current.forEach((t) => window.clearTimeout(t))
    previewTimersRef.current.clear()
  }, [])
  // Dedupe by (name, size, lastModified) — Safari sometimes fires `paste`
  // twice for one Cmd+V; retrying a second upload would create
  // `name (1).png` on the workspace.
  const inFlightAttachRef = useRef<Set<string>>(new Set())
  // Mirror of focusedSessionId for the multi-file loop. Without this, a slow
  // upload that resolves AFTER the user switches panes would write the path
  // into the stale pane (review H1). The ref reads live every iteration.
  const focusedSessionRef = useRef<string | null>(focusedSessionId)
  useEffect(() => { focusedSessionRef.current = focusedSessionId }, [focusedSessionId])

  function dismissPreview(id: string) {
    setPreviews((list) => list.filter((p) => p.id !== id))
  }

  function dismissUpload(id: string) {
    setUploads((list) => list.filter((u) => u.id !== id))
  }

  function cancelUpload(id: string) {
    setUploads((list) => {
      const target = list.find((u) => u.id === id)
      target?.abort.abort()
      return list.filter((u) => u.id !== id)
    })
  }

  // Run a single upload, push chips, and on success drain → preview.
  // Extracted so the chip's Retry button can re-fire without re-traversing
  // attachFiles' dedup gate.
  const runOneUpload = useCallback(
    async (entry: { id: string; file: File; paneIdx: PaneIndex; abort: AbortController }) => {
      if (activeWsId == null) return
      // Best-effort mkdir; ignore failure → upload falls back to ws root.
      const ok = await ensureAttachmentsDir(activeWsId)
      const dir = ok ? ATTACHMENTS_DIR : ''
      try {
        const res = await uploadFile({
          wsId: activeWsId,
          dir,
          file: entry.file,
          signal: entry.abort.signal,
          onProgress: (pct) =>
            setUploads((list) =>
              list.map((u) => (u.id === entry.id ? { ...u, pct } : u)),
            ),
        })
        // Race guard: cancel ✕ may have fired AFTER xhr.onload completed
        // synchronously. Skip side-effects (path insert, preview chip) when
        // the user has already cancelled — they expect a clean abort.
        if (entry.abort.signal.aborted) {
          setUploads((list) => list.filter((u) => u.id !== entry.id))
          return
        }
        const quoted = shellQuote(res.path) + ' '
        const target = focusedSessionRef.current
        if (target) sendFnsRef.current.get(target)?.(quoted)
        // Drop the upload chip, push a preview chip + auto-expire timer.
        setUploads((list) => list.filter((u) => u.id !== entry.id))
        const previewId = entry.id
        setPreviews((list) => [
          ...list,
          { id: previewId, fileName: entry.file.name, file: entry.file, paneIdx: entry.paneIdx },
        ])
        const timerId = window.setTimeout(() => {
          previewTimersRef.current.delete(timerId)
          dismissPreview(previewId)
        }, PREVIEW_AUTO_DISMISS_MS)
        previewTimersRef.current.add(timerId)
      } catch (e) {
        const err = e as Partial<UploadError>
        if (err?.kind === 'cancelled') {
          setUploads((list) => list.filter((u) => u.id !== entry.id))
          return
        }
        setUploads((list) =>
          list.map((u) =>
            u.id === entry.id ? { ...u, state: 'error', error: err as UploadError } : u,
          ),
        )
      }
    },
    [activeWsId],
  )

  const retryUpload = useCallback(
    (id: string) => {
      // Gate: only retry from the error state. Stops a double-click from
      // launching a second in-flight XHR that would push duplicate previews.
      const target = uploadsRef.current.find((u) => u.id === id)
      if (!target || target.state !== 'error') return
      const abort = new AbortController()
      setUploads((list) =>
        list.map((u) =>
          u.id === id ? { ...u, state: 'uploading', pct: 0, error: null, abort } : u,
        ),
      )
      void runOneUpload({ id, file: target.file, paneIdx: target.paneIdx, abort })
    },
    [runOneUpload],
  )

  const attachFiles = useCallback(
    async (files: File[], paneIdx?: PaneIndex) => {
      if (files.length === 0) return
      if (activeWsId == null) {
        setKeybarToast('Pick a workspace before attaching files')
        return
      }
      const targetPane: PaneIndex = paneIdx ?? focusedPane
      for (const file of files) {
        const key = `${file.name}|${file.size}|${file.lastModified}`
        if (inFlightAttachRef.current.has(key)) continue
        inFlightAttachRef.current.add(key)
        const id =
          (typeof crypto !== 'undefined' && 'randomUUID' in crypto)
            ? crypto.randomUUID()
            : `up_${Date.now()}_${Math.random().toString(36).slice(2)}`
        const abort = new AbortController()
        setUploads((list) => [
          ...list,
          { id, fileName: file.name, file, paneIdx: targetPane, pct: 0, state: 'uploading', abort },
        ])
        try {
          await runOneUpload({ id, file, paneIdx: targetPane, abort })
        } finally {
          inFlightAttachRef.current.delete(key)
        }
      }
    },
    [activeWsId, focusedPane, runOneUpload],
  )

  // Document-level paste listener: hijacks paste only when the clipboard
  // carries a file payload, the workspace has an active sessions, and the
  // current focus is not inside an editable surface (rename input, settings
  // popover, modal). Text-only pastes pass through to xterm/inputs untouched.
  useEffect(() => {
    function isInEditable(target: EventTarget | null): boolean {
      const el = target as HTMLElement | null
      if (!el) return false
      const tag = el.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA') return true
      if ((el as HTMLElement).isContentEditable) return true
      return false
    }
    function onPaste(e: ClipboardEvent) {
      if (!e.clipboardData) return
      const items = Array.from(e.clipboardData.items ?? [])
      const fileItems = items.filter((it) => it.kind === 'file')
      if (fileItems.length === 0) return
      // If the user is pasting INTO an input, let it through (image-paste into
      // a text input is unusual, but safer to default to non-hijack).
      if (isInEditable(e.target)) return
      const files = fileItems
        .map((it) => it.getAsFile())
        .filter((f): f is File => f != null)
      if (files.length === 0) return
      e.preventDefault()
      void attachFiles(files)
    }
    document.addEventListener('paste', onPaste)
    return () => document.removeEventListener('paste', onPaste)
  }, [attachFiles])

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
          onNotifyToggle={(sessionId, enabled) => setNotifyOnAgentEnd(sessionId, enabled)}
          onOpenAttach={() => {
            if (activeWsId != null) void ensureAttachmentsDir(activeWsId)
            setAttachModalOpen(true)
          }}
          attachAvailable={activeWsId != null}
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
          onAttachFiles={attachFiles}
          uploads={uploads}
          previews={previews}
          onCancelUpload={cancelUpload}
          onDismissUpload={dismissUpload}
          onRetryUpload={retryUpload}
          onDismissPreview={dismissPreview}
        />
      ) : (
        <div className="relative flex-1 min-h-0 bg-atmosphere flex flex-col items-center justify-center gap-6 px-4 overflow-hidden">
          <span
            aria-hidden
            className="glow-corona"
            style={{ width: '320px', height: '320px', top: '40%', left: '50%', transform: 'translate(-50%, -50%)' }}
          />
          <div className="relative z-10 flex flex-col items-center gap-6 w-full">
            <div className="flex flex-col items-center gap-3 animate-hero-rise">
              <span className="inline-flex items-center justify-center w-14 h-14 rounded-2xl bg-surface-alt border border-border text-accent shadow-[0_10px_30px_-12px_rgba(0,0,0,0.6)]">
                <svg viewBox="0 0 24 24" className="w-7 h-7" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <polyline points="4 6 10 12 4 18" />
                  <line x1="12" y1="18" x2="20" y2="18" />
                </svg>
              </span>
              <div className="text-center">
                <div className="text-base font-semibold text-text-primary tracking-tight">
                  {hasSessions ? 'No pane assigned' : 'No active sessions'}
                </div>
                <div className="text-[13px] text-text-secondary mt-1 max-w-xs">
                  {hasSessions
                    ? 'Pick a tab above to load it, or create a new session.'
                    : 'Each session keeps a persistent shell so you can detach and resume.'}
                </div>
              </div>
            </div>
            <div className="w-full max-w-md animate-hero-rise stagger-2">
              <NewSessionRow onCreate={createSession} />
            </div>
            {err && <div className="text-danger text-xs">{err}</div>}
            {!hasSessions && (
              <p className="text-[11px] text-text-muted text-center max-w-xs leading-relaxed animate-hero-rise stagger-3">
                Tip: start{' '}
                <code className="px-1.5 py-0.5 rounded bg-surface border border-border text-accent text-[10px]" style={{ fontFamily: 'var(--font-mono)' }}>
                  claude code
                </code>{' '}
                and we'll ping your phone when it finishes.
              </p>
            )}
          </div>
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
            onPasteFiles={attachFiles}
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

      {attachModalOpen && activeWsId != null && (
        <FileAttachSheet
          variant="modal"
          wsId={activeWsId}
          dir={ATTACHMENTS_DIR}
          onPathInsert={(path) => {
            const quoted = shellQuote(path) + ' '
            if (focusedSessionId) {
              sendFnsRef.current.get(focusedSessionId)?.(quoted)
            }
          }}
          onClose={() => setAttachModalOpen(false)}
        />
      )}

    </div>
  )
}
