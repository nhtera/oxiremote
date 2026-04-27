import { useEffect, useRef, useState } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'

import { terminalThemes } from '../../lib/terminal-themes'
import type { TerminalPrefs } from '../../lib/terminal-prefs'
import { useTerminalWs, destroyHandle, type SessionHandle } from '../../lib/terminal-ws-hook'

type Props = {
  sessionId: string
  prefs: TerminalPrefs
  isFocused: boolean
  onFocus: () => void
  /** Reports per-session connection state up to the workspace so it can drive
   *  the focused pane's status pill and reconnect modal. */
  onConnectedChange: (sessionId: string, connected: boolean) => void
  onReconnectAttempt: (sessionId: string, attempt: number) => void
  onReconnectExhausted: (sessionId: string) => void
  onError: (msg: string) => void
  /** Workspace keeps a Map<sessionId, sendFn> so the global composer can
   *  forward input to whichever pane is focused. Pass null on unmount. */
  registerSend: (sessionId: string, sendFn: ((data: string) => void) | null) => void
  /** Bumped by the workspace's manual-reconnect button; forwarded to the hook. */
  reconnectNonce: number
}

function debounce<F extends (...args: unknown[]) => void>(fn: F, ms: number) {
  let t: number | undefined
  return (...args: Parameters<F>) => {
    if (t) window.clearTimeout(t)
    t = window.setTimeout(() => fn(...args), ms)
  }
}

export default function XtermPane({
  sessionId, prefs, isFocused, onFocus,
  onConnectedChange, onReconnectAttempt, onReconnectExhausted, onError,
  registerSend, reconnectNonce,
}: Props) {
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

  // Drive the WS for this sessionId via the existing hook (single-active mode).
  useTerminalWs(handlesRef, {
    activeId: mounted ? sessionId : null,
    reconnectNonce,
    onConnected: (c) => onConnectedChange(sessionId, c),
    onError,
    onReconnectAttempt: (n) => onReconnectAttempt(sessionId, n),
    onReconnectExhausted: () => onReconnectExhausted(sessionId),
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

  // Refocus the term when this pane gains focus from a sibling click.
  useEffect(() => {
    if (!isFocused) return
    const h = handlesRef.current.get(sessionId)
    h?.term.focus()
  }, [isFocused, sessionId])

  return (
    <div
      ref={containerRef}
      onClick={onFocus}
      className={`flex-1 min-w-0 min-h-0 overflow-hidden ${
        isFocused ? 'ring-1 ring-inset ring-accent/40' : ''
      }`}
    />
  )
}
