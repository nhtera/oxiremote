import { useEffect, useMemo, useRef, useState } from 'react'
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

function wsUrl(path: string) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

export default function TerminalPage() {
  const [sessions, setSessions] = useState<TerminalSessionMeta[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [isConnected, setIsConnected] = useState(false)
  const [reconnectNonce, setReconnectNonce] = useState(0)

  function reconnectSession() {
    setReconnectNonce((n) => n + 1)
  }

  const containerRef = useRef<HTMLDivElement | null>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const activeIdRef = useRef<string | null>(null)

  const active = useMemo(() => sessions.find((s) => s.id === activeId) ?? null, [sessions, activeId])

  useEffect(() => { activeIdRef.current = activeId }, [activeId])

  async function refreshSessions() {
    setErr(null)
    const res = await fetch('/api/terminal/sessions')
    if (!res.ok) {
      setErr('Not authorized. Pair first at /login.')
      return
    }
    const data = (await res.json()) as TerminalSessionMeta[]
    setSessions(data)
  }

  async function createSession() {
    setErr(null)
    const t = termRef.current
    if (!t) {
      setErr('Terminal not ready')
      return
    }
    const cols = t.cols
    const rows = t.rows
    const body: CreateTerminalSessionRequest = { cols, rows }
    const res = await fetch('/api/terminal/sessions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    if (!res.ok) {
      setErr(await res.text())
      return
    }
    const data = (await res.json()) as CreateTerminalSessionResponse
    await refreshSessions()
    setActiveId(data.id)
  }

  useEffect(() => {
    refreshSessions()
  }, [])

  useEffect(() => {
    const el = containerRef.current
    if (!el) return

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: 'ui-monospace, Consolas, monospace',
      theme: {
        background: '#16171d',
        foreground: '#f3f4f6',
        cursor: '#6cb4ff',
        selectionBackground: '#2a2b3580',
      },
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(el)
    fit.fit()

    termRef.current = term
    fitRef.current = fit

    const debouncedResize = debounce(() => {
      fit.fit()
      const id = activeIdRef.current
      if (id && termRef.current) {
        fetch(`/api/terminal/sessions/${id}/resize`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ cols: termRef.current.cols, rows: termRef.current.rows }),
        })
      }
    }, 200)

    const ro = new ResizeObserver(() => debouncedResize())
    ro.observe(el)

    return () => {
      ro.disconnect()
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
  }, [])

  useEffect(() => {
    if (!activeId) return
    const term = termRef.current
    if (!term) return

    term.clear()
    term.reset()

    const url = wsUrl(`/api/terminal/sessions/${activeId}/ws`)
    const ws = new WebSocket(url)
    wsRef.current = ws

    ws.onopen = () => setIsConnected(true)
    ws.onclose = () => {
      setIsConnected(false)
      term.writeln('\r\n\x1b[33m[disconnected]\x1b[0m')
    }
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data) as WsOut
        if (msg.t === 'output') term.write(msg.data)
        else if (msg.t === 'exit') {
          term.writeln(`\r\n\x1b[90m[process exited: ${msg.code ?? '?'}]\x1b[0m`)
          refreshSessions()
        }
      } catch {}
    }

    const onData = term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ t: 'input', data }))
      }
    })

    return () => {
      onData.dispose()
      ws.close()
      wsRef.current = null
      setIsConnected(false)
    }
  }, [activeId, reconnectNonce])

  return (
    <div className="flex flex-col h-full p-3 md:p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-2 shrink-0">
        <h2 className="text-base font-semibold m-0">Terminal</h2>
        <button onClick={createSession} className="btn-primary text-xs">
          + New
        </button>
        <button onClick={refreshSessions} className="btn-secondary text-xs">
          Refresh
        </button>
        {!isConnected && activeId && (
          <button onClick={reconnectSession} className="btn-secondary text-xs text-warning">
            Reconnect
          </button>
        )}
        {err && <span className="text-danger text-xs">{err}</span>}
      </div>

      <div className="flex gap-3 flex-1 min-h-0">
        {/* Sessions sidebar */}
        <aside className="w-44 md:w-56 shrink-0 overflow-y-auto">
          <div className="text-xs text-text-muted mb-2">Sessions</div>
          <div className="grid gap-1.5">
            {sessions.map((s) => (
              <button
                key={s.id}
                onClick={() => setActiveId(s.id)}
                className={`text-left px-2.5 py-2 rounded-md border text-xs transition-colors ${
                  s.id === activeId
                    ? 'bg-surface-hover border-accent/40 text-text-primary'
                    : 'bg-surface-alt border-border text-text-secondary hover:bg-surface-hover'
                }`}
              >
                <div className="font-medium truncate">{s.name ?? s.id.slice(0, 8)}</div>
                <div className="text-text-muted mt-0.5">{s.status}</div>
              </button>
            ))}
            {sessions.length === 0 && (
              <div className="text-xs text-text-muted">No sessions</div>
            )}
          </div>
        </aside>

        {/* Terminal view */}
        <div className="flex-1 flex flex-col min-w-0">
          <div
            ref={containerRef}
            className="flex-1 border border-border rounded-lg overflow-hidden min-h-[300px]"
          />
          {active && (
            <div className="mt-1.5 text-xs text-text-muted">
              {active.id.slice(0, 8)} &middot; {active.cols}x{active.rows}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
