import { Link } from 'react-router-dom'

// Last-N tail of LogEntry events on the home page so the operator gets a
// glance at "what is the agent doing right now" without leaving for /agent/logs.
// Receives entries from the parent — the home page already owns the SSE
// connection, no second EventSource needed.

interface LogEntry {
  level: 'info' | 'warn' | 'error'
  module: string
  ts: number
  msg: string
}

interface Props {
  entries: LogEntry[]
}

const VISIBLE = 12

const levelClass: Record<LogEntry['level'], string> = {
  info: 'text-text-muted',
  warn: 'text-warning',
  error: 'text-danger',
}

function fmtTs(tsSec: number): string {
  if (!tsSec) return '--:--:--'
  const d = new Date(tsSec * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

export default function RecentLogsCard({ entries }: Props) {
  const visible = entries.slice(-VISIBLE)
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <span className="text-xs text-text-muted">Last {VISIBLE} entries</span>
        <Link
          to="/agent/logs"
          className="text-xs text-accent hover:text-accent-hover"
        >
          Open log viewer →
        </Link>
      </div>
      <div className="font-mono text-[11px] bg-surface-alt border border-border rounded-md p-2 max-h-72 overflow-y-auto">
        {visible.length === 0 ? (
          <div className="text-text-muted">No log entries yet.</div>
        ) : (
          visible.map((e, i) => (
            <div
              key={`${e.ts}-${i}`}
              className="flex items-start gap-2 py-0.5 border-b border-border/30 last:border-0"
            >
              <span className="text-text-muted shrink-0 w-16">{fmtTs(e.ts)}</span>
              <span className={`shrink-0 w-10 font-semibold ${levelClass[e.level]}`}>
                {e.level}
              </span>
              <span className="text-accent shrink-0 w-16 truncate">
                {e.module}
              </span>
              <span className="text-text-primary truncate flex-1" title={e.msg}>
                {e.msg}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

export type { LogEntry }
