import { useMemo, useState } from 'react'
import { useAgentLogsStream, type LogLevel } from '../../hooks/use-agent-logs-stream'
import StatusChip from '../ui/status-chip'

// Embedded log viewer for the agent home's Logs tab. Reuses the shared SSE
// hook so this and /agent/logs stay in sync. Intentionally simpler than the
// full page (no virtualization, capped to 200 visible rows) since this is
// triage-at-a-glance, not deep digging.

const VISIBLE_CAP = 200

export default function InlineLogsPanel() {
  const { entries, connected } = useAgentLogsStream()
  const [query, setQuery] = useState('')
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set(['info', 'warn', 'error']))

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    return entries
      .filter((e) => {
        if (!levels.has(e.level)) return false
        if (q && !e.msg.toLowerCase().includes(q)) return false
        return true
      })
      .slice(-VISIBLE_CAP)
  }, [entries, levels, query])

  function toggleLevel(l: LogLevel) {
    setLevels((prev) => {
      const next = new Set(prev)
      if (next.has(l)) next.delete(l)
      else next.add(l)
      return next
    })
  }

  return (
    <div className="rounded-lg border border-border bg-surface p-3 flex flex-col gap-2">
      <div className="flex items-center gap-2 flex-wrap">
        {(['info', 'warn', 'error'] as LogLevel[]).map((l) => {
          const active = levels.has(l)
          return (
            <button
              key={l}
              onClick={() => toggleLevel(l)}
              aria-pressed={active}
              className={
                'px-2 py-0.5 rounded-full border text-xs font-medium transition-colors ' +
                (active ? levelActiveClass(l) : 'bg-surface-alt text-text-muted border-border')
              }
            >
              {l}
            </button>
          )
        })}
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="search…"
          className="flex-1 min-w-32 bg-surface-alt border border-border rounded-md px-2 py-1 text-xs text-text-primary placeholder:text-text-muted"
        />
        <StatusChip variant={connected ? 'online' : 'warning'}>
          {connected ? 'live' : 'offline'}
        </StatusChip>
        <a
          href="/agent/logs"
          className="text-xs text-accent hover:underline ml-auto"
          title="Open full log viewer"
        >
          Open full
        </a>
      </div>
      <div className="font-mono text-[11px] max-h-[480px] overflow-auto rounded border border-border bg-surface-alt/40">
        {filtered.length === 0 ? (
          <div className="p-3 text-text-muted text-xs text-center">No log entries match.</div>
        ) : (
          filtered.map((e, i) => (
            <div
              key={`${e.ts}-${i}`}
              className="flex items-center gap-2 px-2 py-0.5 border-b border-border/30 last:border-0"
            >
              <span className="text-text-muted shrink-0 w-16">{fmtTs(e.ts)}</span>
              <span className={`shrink-0 w-10 font-semibold ${badgeClass(e.level)}`}>
                {e.level}
              </span>
              <span className="shrink-0 w-16 text-accent truncate">{e.module}</span>
              <span className="text-text-primary truncate flex-1">{e.msg}</span>
            </div>
          ))
        )}
      </div>
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
