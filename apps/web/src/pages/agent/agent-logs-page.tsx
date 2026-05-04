import { useEffect, useMemo, useRef, useState } from 'react'
import { List, type RowComponentProps } from 'react-window'
import { useToast } from '../../components/ui'
import StatusChip from '../../components/ui/status-chip'
import { useAgentLogsStream, type LogEntry, type LogLevel } from '../../hooks/use-agent-logs-stream'

// Localhost-only log viewer. SSE stream + pause buffer live in
// `useAgentLogsStream` so the inline Logs tab on /agent shares the same logic.
//
// Deep-link: clicking a row copies a URL with ?t=<unix-ts> hash.
// Jump-to-latest: when paused with pending entries, a badge button scrolls to
// the bottom and resumes streaming.

const ROW_HEIGHT = 32
const MODULES = ['all', 'tunnel', 'pty', 'files', 'push', 'desktop', 'agent'] as const

export default function AgentLogsPage() {
  const { entries, setEntries, connected, paused, setPaused, pendingCount, resume } =
    useAgentLogsStream()
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set(['info', 'warn', 'error']))
  const [modFilter, setModFilter] = useState<string>('all')
  const [query, setQuery] = useState('')
  const toast = useToast()
  // containerRef is used for ResizeObserver (height) and jump-to-latest scroll.
  const containerRef = useRef<HTMLDivElement>(null)

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    return entries.filter((e) => {
      if (!levels.has(e.level)) return false
      if (modFilter !== 'all' && e.module !== modFilter) return false
      if (q && !e.msg.toLowerCase().includes(q)) return false
      return true
    })
  }, [entries, levels, modFilter, query])

  // On mount: if URL has ?t=<ts>, pause and highlight nearest row.
  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const t = params.get('t')
    if (t) setPaused(true)
  }, [setPaused])

  function toggleLevel(l: LogLevel) {
    setLevels((prev) => {
      const next = new Set(prev)
      if (next.has(l)) next.delete(l)
      else next.add(l)
      return next
    })
  }

  // Copy a deep-link URL with ?t=<unix-ts> to clipboard.
  async function copyRowLink(e: LogEntry) {
    const url = new URL(window.location.href)
    url.searchParams.set('t', String(e.ts))
    try {
      await navigator.clipboard.writeText(url.toString())
      toast.show({ kind: 'success', title: 'Link copied' })
    } catch {
      toast.show({ kind: 'warning', title: 'Clipboard unavailable' })
    }
  }

  function jumpToLatest() {
    resume()
    // Scroll the container div to its bottom — react-window v2 has no ref API.
    if (containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight
    }
  }

  const [height, setHeight] = useState(0)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setHeight(el.clientHeight))
    ro.observe(el)
    setHeight(el.clientHeight)
    return () => ro.disconnect()
  }, [])

  // Highlight the row matching ?t= param from URL.
  const highlightTs = useMemo(() => {
    const params = new URLSearchParams(window.location.search)
    const t = params.get('t')
    return t ? Number(t) : null
  }, [])

  const rowProps = useMemo(
    () => ({ rows: filtered, onCopyLink: copyRowLink, highlightTs }),
    // copyRowLink closes over toast (stable); highlightTs is derived from URL (stable).
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [filtered, highlightTs],
  )

  return (
    <div className="flex flex-col h-dvh p-4 gap-3">
      <header className="shrink-0">
        <div className="flex items-center gap-3">
          <h1 className="text-[length:var(--text-h1)] font-semibold text-text-primary">
            Logs
          </h1>
          <StatusChip variant={connected ? 'online' : 'warning'}>
            {connected ? 'streaming' : 'disconnected'}
          </StatusChip>
          <span className="text-[length:var(--text-meta)] text-text-muted ml-auto">
            {filtered.length} / {entries.length}
          </span>
        </div>
      </header>

      <div className="flex flex-wrap gap-2 items-center shrink-0">
        {(['info', 'warn', 'error'] as LogLevel[]).map((l) => {
          const active = levels.has(l)
          return (
            <button
              key={l}
              onClick={() => toggleLevel(l)}
              aria-pressed={active}
              className={[
                'px-2.5 py-1 rounded-full border text-[length:var(--text-meta)] font-medium leading-none transition-colors',
                active ? levelActiveClass(l) : 'bg-surface-alt text-text-muted border-border hover:bg-surface-hover',
              ].join(' ')}
            >
              {l}
            </button>
          )
        })}
        <select
          value={modFilter}
          onChange={(e) => setModFilter(e.target.value)}
          className="bg-surface border border-border rounded-md px-2 py-1 text-[length:var(--text-meta)] text-text-primary"
        >
          {MODULES.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="search…"
          className="flex-1 min-w-40 bg-surface border border-border rounded-md px-2 py-1 text-[length:var(--text-meta)] text-text-primary placeholder:text-text-muted"
        />

        {paused ? (
          <button
            onClick={jumpToLatest}
            className="relative text-[length:var(--text-meta)] py-1 px-2.5 rounded-md border border-accent/30 bg-accent/15 text-accent hover:bg-accent/25 transition-colors"
          >
            Resume
            {pendingCount > 0 && (
              <span className="absolute -top-1.5 -right-1.5 inline-flex items-center justify-center min-w-4 h-4 px-1 rounded-full bg-accent text-white text-[9px] font-bold leading-none">
                {pendingCount > 99 ? '99+' : pendingCount}
              </span>
            )}
          </button>
        ) : (
          <button
            onClick={() => setPaused(true)}
            className="btn-secondary text-[length:var(--text-meta)] py-1 px-2.5"
          >
            Pause
          </button>
        )}

        <button
          onClick={() => setEntries([])}
          className="btn-secondary text-[length:var(--text-meta)] py-1 px-2.5"
        >
          Clear
        </button>
      </div>

      <div
        ref={containerRef}
        className="flex-1 min-h-0 border border-border rounded-md bg-surface font-mono text-[length:var(--text-mono)] relative"
      >
        {height > 0 && (
          <List
            rowComponent={LogRow}
            rowCount={filtered.length}
            rowHeight={ROW_HEIGHT}
            rowProps={rowProps}
            overscanCount={10}
            style={{ height }}
          />
        )}
      </div>
    </div>
  )
}

type RowProps = {
  rows: LogEntry[]
  onCopyLink: (e: LogEntry) => void
  highlightTs: number | null
}

function LogRow({ index, style, rows, onCopyLink, highlightTs }: RowComponentProps<RowProps>) {
  const e = rows[index]
  if (!e) return null
  const isHighlighted = highlightTs !== null && e.ts === highlightTs
  return (
    <div
      style={style}
      onClick={() => onCopyLink(e)}
      title="Click to copy link to this row"
      className={[
        'flex items-center gap-2 px-2 border-b border-border/30 cursor-pointer hover:bg-surface-alt transition-colors',
        isHighlighted ? 'bg-accent/10 border-l-2 border-l-accent' : '',
      ].join(' ')}
    >
      <span className="text-text-muted shrink-0 w-20">{fmtTs(e.ts)}</span>
      <span className={`shrink-0 w-12 font-semibold ${badgeClass(e.level)}`}>
        {e.level}
      </span>
      <span className="shrink-0 w-20 text-accent truncate">{e.module}</span>
      <span className="text-text-primary truncate flex-1">{e.msg}</span>
    </div>
  )
}

function badgeClass(l: LogLevel): string {
  if (l === 'error') return 'text-danger'
  if (l === 'warn') return 'text-warning'
  return 'text-text-muted'
}

function levelActiveClass(l: LogLevel): string {
  if (l === 'error') return 'bg-danger/10 text-danger border-danger/30'
  if (l === 'warn') return 'bg-warning/10 text-warning border-warning/30'
  return 'bg-accent/10 text-accent border-accent/30'
}

function fmtTs(tsSec: number): string {
  if (!tsSec) return '--:--:--'
  const d = new Date(tsSec * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

