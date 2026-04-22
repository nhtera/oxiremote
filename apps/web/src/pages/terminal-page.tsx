import { useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'

function debounce<F extends (...args: any[]) => void>(fn: F, waitMs: number) {
  let t: number | undefined
  return (...args: Parameters<F>) => {
    if (t) window.clearTimeout(t)
    t = window.setTimeout(() => fn(...args), waitMs)
  }
}

type TerminalSessionMeta = {
  id: string
  name: string | null
  created_at: number
  last_seen_at: number
  cols: number
  rows: number
  status: string
  exit_code: number | null
}

type WsOut = { t: 'output'; data: string } | { t: 'exit'; code: number | null }
type CreateTerminalSessionResponse = { id: string }
type CreateTerminalSessionRequest = {
  name?: string
  cwd?: string
  command?: string[]
  cols: number
  rows: number
}

type SessionHandle = {
  term: Terminal
  fit: FitAddon
  ws: WebSocket | null
  connected: boolean
}

const TERM_THEME = {
  background: '#16171d',
  foreground: '#f3f4f6',
  cursor: '#6cb4ff',
  selectionBackground: '#2a2b3580',
}

const CONTROL_KEYS = [
  { label: 'Ctrl+C', value: '' },
  { label: 'Esc', value: '' },
  { label: 'Tab', value: '\t' },
  { label: '↑', value: '[A' },
  { label: '↓', value: '[B' },
  { label: '←', value: '[D' },
  { label: '→', value: '[C' },
]

function wsUrl(path: string) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

export default function TerminalPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const [sessions, setSessions] = useState<TerminalSessionMeta[]>([])
  const [activeId, setActiveId] = useState<string | null>(searchParams.get('session'))
  const [err, setErr] = useState<string | null>(null)
  const [isConnected, setIsConnected] = useState(false)
  const [reconnectNonce, setReconnectNonce] = useState(0)

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

  function setSelectedSession(id: string) {
    setActiveId(id)
    window.localStorage.setItem('oxi:last-terminal-session', id)
  }

  function reconnectSession() {
    if (!activeId) return
    const handle = handlesRef.current.get(activeId)
    if (handle?.ws) {
      handle.ws.close()
      handle.ws = null
    }
    setReconnectNonce((n) => n + 1)
  }

  function getOrCreateHandle(id: string): SessionHandle {
    const existing = handlesRef.current.get(id)
    if (existing) return existing

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: 'ui-monospace, Consolas, monospace',
      theme: TERM_THEME,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)

    const handle: SessionHandle = { term, fit, ws: null, connected: false }
    handlesRef.current.set(id, handle)
    return handle
  }

  function destroyHandle(id: string) {
    const handle = handlesRef.current.get(id)
    if (!handle) return
    handle.ws?.close()
    handle.term.dispose()
    handlesRef.current.delete(id)
  }

  async function refreshSessions() {
    setErr(null)
    const res = await fetch('/api/terminal/sessions', { credentials: 'include' })
    if (!res.ok) {
      setErr('Not authorized. Pair first at /login.')
      return
    }
    const data = (await res.json()) as TerminalSessionMeta[]
    setSessions(data)

    const remembered = searchParams.get('session') || window.localStorage.getItem('oxi:last-terminal-session')
    const rememberedSession = remembered ? data.find((s) => s.id === remembered) ?? null : null
    const bestId = rememberedSession?.status === 'running'
      ? rememberedSession.id
      : data.find((s) => s.status === 'running')?.id ?? rememberedSession?.id ?? data[0]?.id ?? null

    if (bestId !== activeIdRef.current) {
      setActiveId(bestId)
      if (bestId) window.localStorage.setItem('oxi:last-terminal-session', bestId)
    }

    const ids = new Set(data.map((s) => s.id))
    for (const id of handlesRef.current.keys()) {
      if (!ids.has(id)) destroyHandle(id)
    }
  }

  async function createSession() {
    setErr(null)
    const activeHandle = activeId ? handlesRef.current.get(activeId) : null
    const cols = activeHandle?.term.cols ?? 80
    const rows = activeHandle?.term.rows ?? 24
    const body: CreateTerminalSessionRequest = { cols, rows }
    const res = await fetch('/api/terminal/sessions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: JSON.stringify(body),
    })
    if (!res.ok) {
      setErr(await res.text())
      return
    }
    const data = (await res.json()) as CreateTerminalSessionResponse
    await refreshSessions()
    setSelectedSession(data.id)
  }

  function sendInput(data: string) {
    if (!activeId) return
    const handle = handlesRef.current.get(activeId)
    if (handle?.ws?.readyState === WebSocket.OPEN) {
      handle.ws.send(JSON.stringify({ t: 'input', data }))
      handle.term.focus()
    }
  }

  async function closeActiveSession() {
    if (!activeId) return
    const res = await fetch(`/api/terminal/sessions/${activeId}/close`, {
      method: 'POST',
      credentials: 'include',
    })
    if (!res.ok) {
      setErr('Failed to close session')
      return
    }
    await refreshSessions()
  }

  useEffect(() => {
    refreshSessions()
    return () => {
      for (const id of handlesRef.current.keys()) destroyHandle(id)
    }
  }, [])

  useEffect(() => {
    const el = containerRef.current
    if (!el || !activeId) return

    const handle = getOrCreateHandle(activeId)
    el.replaceChildren()
    handle.term.open(el)
    handle.fit.fit()
    handle.term.focus()

    setIsConnected(handle.connected)

    const debouncedResize = debounce(() => {
      handle.fit.fit()
      if (activeIdRef.current === activeId) {
        fetch(`/api/terminal/sessions/${activeId}/resize`, {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ cols: handle.term.cols, rows: handle.term.rows }),
        })
      }
    }, 200)

    const ro = new ResizeObserver(() => debouncedResize())
    ro.observe(el)

    return () => {
      ro.disconnect()
    }
  }, [activeId])

  useEffect(() => {
    if (!activeId) return
    const handle = handlesRef.current.get(activeId)
    if (!handle) return

    const sessionId = activeId

    if (!handle.ws || handle.ws.readyState > WebSocket.OPEN) {
      if (handle.ws) {
        handle.ws.close()
        handle.ws = null
      }

      const url = wsUrl(`/api/terminal/sessions/${sessionId}/ws`)
      const ws = new WebSocket(url)
      handle.ws = ws

      ws.onopen = () => {
        handle.connected = true
        if (activeIdRef.current === sessionId) setIsConnected(true)
      }
      ws.onclose = () => {
        handle.connected = false
        if (activeIdRef.current === sessionId) {
          setIsConnected(false)
          handle.term.writeln('\r\n\x1b[33m[disconnected]\x1b[0m')
        }
      }
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data) as WsOut
          if (msg.t === 'output') handle.term.write(msg.data)
          else if (msg.t === 'exit') {
            handle.term.writeln(`\r\n\x1b[90m[process exited: ${msg.code ?? '?'}]\x1b[0m`)
            refreshSessions()
          }
        } catch {}
      }
    } else {
      setIsConnected(handle.connected)
    }

    const onData = handle.term.onData((data) => {
      if (handle.ws?.readyState === WebSocket.OPEN) {
        handle.ws.send(JSON.stringify({ t: 'input', data }))
      }
    })

    return () => {
      onData.dispose()
    }
  }, [activeId, reconnectNonce])

  return (
    <div className="flex flex-col h-full p-3 md:p-4 gap-3">
      <div className="flex flex-wrap items-center gap-2 shrink-0 sticky top-0 z-10 bg-surface/95 backdrop-blur py-1">
        <h2 className="text-base font-semibold m-0 mr-auto">Terminal</h2>
        <span className={`text-[11px] px-2 py-1 rounded-full border ${isConnected ? 'text-success border-success/30 bg-success/10' : 'text-warning border-warning/30 bg-warning/10'}`}>
          {activeId ? (isConnected ? 'Connected' : 'Disconnected') : 'No session'}
        </span>
        <button onClick={createSession} className="btn-primary text-xs min-h-10">+ New</button>
        <button onClick={refreshSessions} className="btn-secondary text-xs min-h-10">Refresh</button>
        {!isConnected && activeId && (
          <button onClick={reconnectSession} className="btn-secondary text-xs min-h-10 text-warning">Reconnect</button>
        )}
        {activeId && (
          <button onClick={closeActiveSession} className="btn-danger text-xs min-h-10">Close</button>
        )}
        {err && <span className="text-danger text-xs basis-full">{err}</span>}
      </div>

      <div className="flex flex-col md:flex-row gap-3 flex-1 min-h-0">
        <aside className="w-full md:w-56 shrink-0 overflow-hidden">
          <div className="text-xs text-text-muted mb-2">Sessions</div>
          <div className="flex md:grid overflow-x-auto gap-1.5 pb-1">
            {sessions.map((s) => (
              <button
                key={s.id}
                onClick={() => setSelectedSession(s.id)}
                className={`text-left px-2.5 py-2 rounded-md border text-xs transition-colors min-w-[120px] md:min-w-0 ${
                  s.id === activeId
                    ? 'bg-surface-hover border-accent/40 text-text-primary'
                    : 'bg-surface-alt border-border text-text-secondary hover:bg-surface-hover'
                }`}
              >
                <div className="font-medium truncate">{s.name ?? s.id.slice(0, 8)}</div>
                <div className="text-text-muted mt-0.5 truncate">{s.status}</div>
              </button>
            ))}
            {sessions.length === 0 && <div className="text-xs text-text-muted">No sessions</div>}
          </div>
        </aside>

        <div className="flex-1 flex flex-col min-w-0 gap-2">
          <div
            ref={containerRef}
            onClick={() => handlesRef.current.get(activeId ?? '')?.term.focus()}
            className="flex-1 border border-border rounded-lg overflow-hidden min-h-[50dvh] md:min-h-[300px]"
          />

          <div className="flex flex-wrap gap-1.5">
            {CONTROL_KEYS.map((key) => (
              <button
                key={key.label}
                onClick={() => sendInput(key.value)}
                className="btn-secondary text-xs py-1 px-2"
              >
                {key.label}
              </button>
            ))}
            <button onClick={() => navigator.clipboard.readText().then(sendInput).catch(() => setErr('Paste unavailable'))} className="btn-secondary text-xs py-1 px-2">
              Paste
            </button>
            <button onClick={() => sendInput('clear\n')} className="btn-secondary text-xs py-1 px-2">
              Clear
            </button>
          </div>

          {active && (
            <div className="text-xs text-text-muted">
              {active.id.slice(0, 8)} · {active.cols}x{active.rows} · {active.status}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
