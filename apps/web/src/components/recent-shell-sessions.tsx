import { useEffect, useState } from 'react'
import { relativeTimeMs } from '../lib/device-label'

// Phase 06c / H11. Surfaces ShellOpened/ShellClosed events from the agent's
// bounded ring buffer (`GET /api/agent/sessions/recent`) plus live SSE updates
// from `/api/agent/events` so the operator sees an audit trail of every shell
// that has been spawned. The pull endpoint backfills the last 50 events on
// mount so a tab opened mid-session still shows prior shells (broadcast bus
// has no replay).

type RecentEvent =
  | {
      type: 'shell_opened'
      device_id: string
      session_id: string
      command: string
      cwd?: string
      ts: number
    }
  | {
      type: 'shell_closed'
      device_id: string
      session_id: string
      exit_code?: number
      ts: number
    }

const MAX_RENDERED = 25

function eventKey(ev: RecentEvent): string {
  return `${ev.session_id}:${ev.type}:${ev.ts}`
}

function shortDevice(id: string): string {
  return id.length > 10 ? `${id.slice(0, 10)}…` : id
}

function shortenCommand(cmd: string): string {
  // Trim long shell paths to the basename for compactness; keep the full
  // string in the title attribute so operators can still inspect it.
  const first = cmd.split(' ')[0]
  const base = first.split('/').pop() ?? first
  const rest = cmd.slice(first.length).trim()
  return rest ? `${base} ${rest}` : base
}

export default function RecentShellSessions() {
  const [events, setEvents] = useState<RecentEvent[]>([])
  const [error, setError] = useState(false)
  const [, setTick] = useState(0)

  // Backfill on mount via the ring-buffer pull endpoint. Merge (not replace)
  // with whatever arrived from SSE before this resolved — otherwise events
  // delivered in the fetch-in-flight window are silently lost.
  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/sessions/recent')
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json() as Promise<RecentEvent[]>
      })
      .then((data) => {
        if (cancelled) return
        // Server returns oldest-first; render newest-first. Dedup against
        // anything the live SSE handler already pushed.
        const backfill = [...data].reverse()
        setEvents((prev) => {
          const seen = new Set(prev.map(eventKey))
          const merged = [
            ...prev,
            ...backfill.filter((e) => !seen.has(eventKey(e))),
          ]
          // Render newest-first, then cap.
          merged.sort((a, b) => b.ts - a.ts)
          return merged.slice(0, MAX_RENDERED)
        })
      })
      .catch(() => {
        if (!cancelled) setError(true)
      })
    return () => {
      cancelled = true
    }
  }, [])

  // Live updates via SSE. The home page already opens its own EventSource for
  // device/tunnel events; the audit log is a separate concern so we keep its
  // subscription self-contained here rather than threading a shared bus. The
  // M2 follow-up (single-EventSource refactor) is tracked in the backlog.
  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type?: string }
        if (ev.type !== 'shell_opened' && ev.type !== 'shell_closed') return
        const event = ev as RecentEvent
        setEvents((prev) => {
          // Dedup against backfill: an SSE event delivered between mount and
          // backfill-resolve will be re-emitted by the ring-buffer snapshot.
          if (prev.some((p) => eventKey(p) === eventKey(event))) return prev
          return [event, ...prev].slice(0, MAX_RENDERED)
        })
      } catch {
        // ignore malformed frames
      }
    }
    return () => es.close()
  }, [])

  // Re-render relative times every 10 s without re-fetching.
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 10_000)
    return () => window.clearInterval(id)
  }, [])

  return (
    <section className="rounded-xl border border-border bg-surface p-4 space-y-3">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-[length:var(--text-h3)] font-semibold text-text-primary">
          Recent Shells
        </h2>
        <span className="text-[length:var(--text-meta)] text-text-muted">
          {events.length} event{events.length !== 1 ? 's' : ''}
        </span>
      </div>

      {error && (
        <div className="text-[length:var(--text-meta)] text-danger">
          Could not load shell audit log.
        </div>
      )}

      {!error && events.length === 0 && (
        <div className="text-[length:var(--text-meta)] text-text-muted">
          No shells spawned yet.
        </div>
      )}

      {events.map((ev) => {
        const tsMs = ev.ts * 1000
        const key = eventKey(ev)
        if (ev.type === 'shell_opened') {
          return (
            <div
              key={key}
              className="flex items-center gap-2 border border-border rounded-lg px-3 py-2 text-sm"
            >
              <span className="shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded bg-success/15 border border-success/40 text-success">
                opened
              </span>
              <span
                className="flex-1 min-w-0 truncate font-mono text-xs text-text-primary"
                title={`${ev.command}${ev.cwd ? `  (cwd: ${ev.cwd})` : ''}`}
              >
                {shortenCommand(ev.command)}
              </span>
              <span className="shrink-0 text-[length:var(--text-meta)] text-text-muted" title={ev.device_id}>
                {shortDevice(ev.device_id)}
              </span>
              <span className="shrink-0 text-[length:var(--text-meta)] text-text-muted">
                {relativeTimeMs(tsMs)}
              </span>
            </div>
          )
        }
        return (
          <div
            key={key}
            className="flex items-center gap-2 border border-border rounded-lg px-3 py-2 text-sm"
          >
            <span className="shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded bg-surface-alt border border-border text-text-muted">
              closed
            </span>
            <span
              className="flex-1 min-w-0 truncate font-mono text-xs text-text-muted"
              title={ev.session_id}
            >
              session {ev.session_id.slice(0, 8)}
              {typeof ev.exit_code === 'number' ? ` · exit ${ev.exit_code}` : ''}
            </span>
            <span className="shrink-0 text-[length:var(--text-meta)] text-text-muted" title={ev.device_id}>
              {shortDevice(ev.device_id)}
            </span>
            <span className="shrink-0 text-[length:var(--text-meta)] text-text-muted">
              {relativeTimeMs(tsMs)}
            </span>
          </div>
        )
      })}
    </section>
  )
}
