import { useCallback, useEffect, useRef, useState } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'

import { terminalThemes } from '../../lib/terminal-themes'
import type { TerminalPrefs } from '../../lib/terminal-prefs'
import { useTerminalWs, destroyHandle, type ReconnectPhase, type SessionHandle } from '../../lib/terminal-ws-hook'
import { useVisualViewport } from '../../hooks/use-visual-viewport'
import type { PaneIndex } from '../../state/terminal-store'
import { UploadChip } from '../upload-chip'
import { UploadPreviewChip } from '../upload-preview-chip'
import type { PaneUpload, PanePreview } from './multi-pane-grid'

type Props = {
  sessionId: string
  paneIdx: PaneIndex
  prefs: TerminalPrefs
  isFocused: boolean
  onFocus: () => void
  /** Reports per-session connection state up to the workspace so it can drive
   *  the focused pane's status pill and reconnect modal. */
  onConnectedChange: (sessionId: string, connected: boolean) => void
  onReconnectAttempt: (sessionId: string, attempt: number) => void
  onReconnectPhase: (sessionId: string, phase: ReconnectPhase) => void
  onError: (msg: string) => void
  /** Workspace keeps a Map<sessionId, sendFn> so the global composer can
   *  forward input to whichever pane is focused. Pass null on unmount. */
  registerSend: (sessionId: string, sendFn: ((data: string) => void) | null) => void
  /** Optional companion to registerSend — exposes the focused term's current
   *  selection text so the mobile keybar can offer a Copy button. */
  registerGetSelection?: (sessionId: string, fn: (() => string) | null) => void
  /** Bumped by the workspace's manual-reconnect button; forwarded to the hook. */
  reconnectNonce: number
  /** Called when the user drops files onto this pane. Page handles upload +
   *  inserts the path into the focused pane via sendInput. Optional so legacy
   *  hosts that don't wire it stay intact. */
  onAttachFiles?: (files: File[], paneIdx?: PaneIndex) => void
  /** Active uploads + recent previews scoped to this pane. */
  uploads?: PaneUpload[]
  previews?: PanePreview[]
  onCancelUpload?: (id: string) => void
  onDismissUpload?: (id: string) => void
  onRetryUpload?: (id: string) => void
  onDismissPreview?: (id: string) => void
}

function debounce<F extends (...args: unknown[]) => void>(fn: F, ms: number) {
  let t: number | undefined
  return (...args: Parameters<F>) => {
    if (t) window.clearTimeout(t)
    t = window.setTimeout(() => fn(...args), ms)
  }
}

export default function XtermPane({
  sessionId, paneIdx, prefs, isFocused, onFocus,
  onConnectedChange, onReconnectAttempt, onReconnectPhase, onError,
  registerSend, registerGetSelection, reconnectNonce,
  onAttachFiles,
  uploads, previews,
  onCancelUpload, onDismissUpload, onRetryUpload, onDismissPreview,
}: Props) {
  // Drop-target highlight while the user drags files over this pane.
  const [dropActive, setDropActive] = useState(false)
  // Counter so nested children's dragenter/dragleave don't flicker the overlay.
  const dragDepthRef = useRef(0)
  const containerRef = useRef<HTMLDivElement | null>(null)
  // Each pane owns its own handles map keyed by its single sessionId. Keeps
  // the existing useTerminalWs hook (designed for a single activeId) reusable
  // without broadening it to N sessions.
  const handlesRef = useRef<Map<string, SessionHandle>>(new Map())
  const prefsRef = useRef(prefs)
  const [mounted, setMounted] = useState(false)
  useEffect(() => { prefsRef.current = prefs }, [prefs])

  // Create xterm + handle on first mount of this sessionId.
  useEffect(() => {
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
    handlesRef.current.set(sessionId, {
      term, fit, ws: null, connected: false,
      reconnectTimer: null, reconnectAttempt: 0, closedByUser: false,
      phase: 'fast', slowAttempt: 0, lastProbeAt: 0, scheduleEpoch: 0,
      everOpenedThisCycle: false, fastRetryUsed: false,
    })
    setMounted(true)
    return () => {
      destroyHandle(handlesRef, sessionId)
      setMounted(false)
    }
  }, [sessionId])

  // Mount xterm into the pane container + size + resize-observer.
  useEffect(() => {
    if (!mounted) return
    const el = containerRef.current
    if (!el) return
    const handle = handlesRef.current.get(sessionId)
    if (!handle) return
    el.replaceChildren()
    handle.term.open(el)
    handle.fit.fit()
    if (isFocused) handle.term.focus()

    // Suppress iOS auto-correct / auto-capitalise / predictive bar on the
    // hidden helper textarea — it otherwise mangles shell input by inserting
    // smart quotes, capital first letters, or text suggestions.
    const helper = el.querySelector<HTMLTextAreaElement>('.xterm-helper-textarea')
    if (helper) {
      helper.setAttribute('autocomplete', 'off')
      helper.setAttribute('autocorrect', 'off')
      helper.setAttribute('autocapitalize', 'off')
      helper.setAttribute('spellcheck', 'false')
      helper.setAttribute('inputmode', 'text')
    }

    const debouncedResize = debounce(() => {
      try { handle.fit.fit() } catch { /* container detached */ }
      fetch(`/api/terminal/sessions/${sessionId}/resize`, {
        method: 'POST', credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ cols: handle.term.cols, rows: handle.term.rows }),
      }).catch(() => { /* resize is best-effort */ })
    }, 200)
    const ro = new ResizeObserver(() => debouncedResize())
    ro.observe(el)
    return () => { ro.disconnect() }
  }, [mounted, sessionId, isFocused])

  // Apply pref changes live.
  useEffect(() => {
    const handle = handlesRef.current.get(sessionId)
    if (!handle) return
    const theme = terminalThemes[prefs.theme] ?? terminalThemes.default
    handle.term.options.theme = theme
    handle.term.options.fontSize = prefs.fontSize
    handle.term.options.cursorStyle = prefs.cursorStyle
    try { handle.fit.fit() } catch { /* container detached */ }
  }, [prefs.theme, prefs.fontSize, prefs.cursorStyle, sessionId])

  // Copy-on-select: when the user finishes a selection, copy it to the
  // clipboard. xterm.js fires onSelectionChange on every range mutation,
  // including the final commit, so we copy the latest snapshot. Best-effort —
  // navigator.clipboard requires a secure context (the tunnel always is).
  useEffect(() => {
    if (!prefs.copyOnSelect) return
    const handle = handlesRef.current.get(sessionId)
    if (!handle) return
    const sub = handle.term.onSelectionChange(() => {
      const sel = handle.term.getSelection()
      if (!sel) return
      navigator.clipboard?.writeText(sel).catch(() => {
        /* clipboard denied / not focused — silent no-op */
      })
    })
    return () => sub.dispose()
  }, [prefs.copyOnSelect, sessionId, mounted])

  // Drive the WS for this sessionId via the existing hook (single-active mode).
  useTerminalWs(handlesRef, {
    activeId: mounted ? sessionId : null,
    reconnectNonce,
    onConnected: (c) => onConnectedChange(sessionId, c),
    onError,
    onReconnectAttempt: (n) => onReconnectAttempt(sessionId, n),
    onReconnectPhase: (phase) => onReconnectPhase(sessionId, phase),
  })

  // Expose sendInput to the workspace so the bottom composer can dispatch
  // to this pane while it's focused.
  useEffect(() => {
    const send = (data: string) => {
      const h = handlesRef.current.get(sessionId)
      if (h?.ws?.readyState === WebSocket.OPEN) {
        h.ws.send(JSON.stringify({ t: 'input', data }))
        h.term.focus()
      }
    }
    registerSend(sessionId, send)
    return () => registerSend(sessionId, null)
  }, [sessionId, registerSend])

  // Expose getSelection so the workspace's mobile keybar can offer a
  // "Copy selection" action against whichever pane is focused.
  useEffect(() => {
    if (!registerGetSelection) return
    const getter = () => {
      const h = handlesRef.current.get(sessionId)
      return h?.term.getSelection() ?? ''
    }
    registerGetSelection(sessionId, getter)
    return () => registerGetSelection(sessionId, null)
  }, [sessionId, registerGetSelection])

  // Refocus the term when this pane gains focus from a sibling click.
  useEffect(() => {
    if (!isFocused) return
    const h = handlesRef.current.get(sessionId)
    h?.term.focus()
  }, [isFocused, sessionId])

  // iOS Safari soft-keyboard fix: when the visual viewport shrinks, the grid
  // layout's `100dvh` may not contract on older Safari, leaving the cursor
  // hidden behind the keyboard. A direct fit() pass against the new viewport
  // height makes xterm recompute rows so the prompt stays in view.
  const refitOnViewport = useCallback(() => {
    const h = handlesRef.current.get(sessionId)
    if (!h) return
    try { h.fit.fit() } catch { /* container detached */ }
  }, [sessionId])
  useVisualViewport(refitOnViewport)

  // Drag-drop wiring. Only kicks in when the parent passed `onAttachFiles`
  // (workspace-page does; legacy hosts may not). `dataTransfer.types`
  // includes 'Files' when the user is dragging from the OS file manager;
  // ignore in-page text drags (xterm's own text-drop behaviour stays).
  const hasFilesPayload = useCallback((dt: DataTransfer | null): boolean => {
    if (!dt) return false
    return Array.from(dt.types ?? []).includes('Files')
  }, [])

  // Window-level safety net: Escape mid-drag, drag exit to a non-pane region,
  // or a Firefox quirk where dragleave's dataTransfer.types is empty can all
  // strand the depth counter > 0 and leave the overlay visible. `dragend` on
  // the window force-resets so the next drag starts clean (review H2).
  useEffect(() => {
    if (!onAttachFiles) return
    const reset = () => {
      dragDepthRef.current = 0
      setDropActive(false)
    }
    window.addEventListener('dragend', reset)
    return () => window.removeEventListener('dragend', reset)
  }, [onAttachFiles])

  const onDragEnter = useCallback(
    (e: React.DragEvent) => {
      if (!onAttachFiles || !hasFilesPayload(e.dataTransfer)) return
      e.preventDefault()
      dragDepthRef.current += 1
      setDropActive(true)
    },
    [onAttachFiles, hasFilesPayload],
  )
  const onDragOver = useCallback(
    (e: React.DragEvent) => {
      if (!onAttachFiles || !hasFilesPayload(e.dataTransfer)) return
      e.preventDefault()
      e.dataTransfer.dropEffect = 'copy'
    },
    [onAttachFiles, hasFilesPayload],
  )
  const onDragLeave = useCallback(
    (e: React.DragEvent) => {
      if (!onAttachFiles || !hasFilesPayload(e.dataTransfer)) return
      e.preventDefault()
      dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
      if (dragDepthRef.current === 0) setDropActive(false)
    },
    [onAttachFiles, hasFilesPayload],
  )
  const onDrop = useCallback(
    (e: React.DragEvent) => {
      if (!onAttachFiles || !hasFilesPayload(e.dataTransfer)) return
      e.preventDefault()
      e.stopPropagation()
      dragDepthRef.current = 0
      setDropActive(false)
      const files = Array.from(e.dataTransfer.files)
      if (files.length > 0) onAttachFiles(files, paneIdx)
    },
    [onAttachFiles, hasFilesPayload, paneIdx],
  )

  return (
    <div
      className={`relative flex-1 min-w-0 min-h-0 overflow-hidden ${
        isFocused ? 'ring-1 ring-inset ring-accent/40' : ''
      }`}
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      <div
        ref={containerRef}
        onClick={onFocus}
        className="absolute inset-0"
      />
      {dropActive && (
        <div
          className="absolute inset-0 z-10 flex items-center justify-center bg-accent/15 border-2 border-dashed border-accent/60 pointer-events-none"
          aria-hidden
        >
          <div className="px-3 py-1.5 rounded-md bg-surface border border-accent/40 text-accent text-sm font-medium shadow">
            Drop to attach
          </div>
        </div>
      )}
      {uploads && uploads.length > 0 && (
        <div className="pointer-events-none absolute right-2 top-2 z-20 flex flex-col gap-1.5">
          {uploads.slice(0, 3).map((u) => (
            <UploadChip
              key={u.id}
              fileName={u.fileName}
              pct={u.pct}
              state={u.state}
              error={u.error}
              onCancel={onCancelUpload ? () => onCancelUpload(u.id) : undefined}
              onDismiss={onDismissUpload ? () => onDismissUpload(u.id) : undefined}
              onRetry={onRetryUpload ? () => onRetryUpload(u.id) : undefined}
            />
          ))}
          {uploads.length > 3 && (
            <div className="pointer-events-auto self-end rounded-full border border-border bg-surface px-2 py-0.5 text-[10px] text-text-muted">
              +{uploads.length - 3} more
            </div>
          )}
        </div>
      )}
      {previews && previews.length > 0 && (
        <div className="pointer-events-none absolute left-2 top-2 z-20 flex flex-col gap-1.5">
          {previews.slice(0, 3).map((p) => (
            <UploadPreviewChip
              key={p.id}
              fileName={p.fileName}
              file={p.file}
              onDismiss={
                onDismissPreview ? () => onDismissPreview(p.id) : () => undefined
              }
            />
          ))}
        </div>
      )}
    </div>
  )
}
