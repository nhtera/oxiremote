import { useEffect, useState } from 'react'
import { prettyDeviceLabel, relativeTimeSec } from '../lib/device-label'

// Key usage event from GET /api/agent/keys/recent-usage (last 10)
interface KeyUsageEvent {
  kind: 'otk_used' | 'key_verified'
  device_label: string
  ip: string
  at: number
}

const POLL_MS = 30_000

function kindLabel(kind: KeyUsageEvent['kind']): string {
  return kind === 'otk_used' ? 'OTK used' : 'Key verified'
}

function kindClass(kind: KeyUsageEvent['kind']): string {
  return kind === 'otk_used'
    ? 'bg-accent/10 text-accent border-accent/30'
    : 'bg-success/10 text-success border-success/30'
}

// Displays last 10 OTK-used / key-verified events. Polls every 30 s —
// security audit log, not a live feed. Device labels truncated to 24 chars.
export default function RecentKeyUsage() {
  const [events, setEvents] = useState<KeyUsageEvent[]>([])
  const [error, setError] = useState(false)

  useEffect(() => {
    let cancelled = false

    const poll = async () => {
      try {
        const res = await fetch('/api/agent/keys/recent-usage')
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
        const data = (await res.json()) as KeyUsageEvent[]
        if (!cancelled) {
          setEvents(data)
          setError(false)
        }
      } catch {
        if (!cancelled) setError(true)
      }
    }

    void poll()
    const id = window.setInterval(poll, POLL_MS)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [])

  return (
    <section className="rounded-xl border border-border bg-surface p-4 space-y-3">
      <h2 className="text-[length:var(--text-h3)] font-semibold text-text-primary">
        Recent Key Usage
      </h2>

      {error && (
        <div className="text-[length:var(--text-meta)] text-text-muted">
          Could not load key usage history.
        </div>
      )}

      {!error && events.length === 0 && (
        <div className="text-[length:var(--text-meta)] text-text-muted">
          No key events recorded yet.
        </div>
      )}

      {events.length > 0 && (
        <div className="space-y-1.5">
          {events.map((ev, i) => {
            const label = prettyDeviceLabel(ev.device_label)
            return (
              <div
                key={i}
                className="flex items-center gap-2 text-[length:var(--text-meta)] border-b border-border/40 pb-1.5 last:border-0 last:pb-0"
              >
                <span
                  className={`shrink-0 px-1.5 py-0.5 rounded border text-[10px] font-medium leading-none ${kindClass(ev.kind)}`}
                >
                  {kindLabel(ev.kind)}
                </span>
                <span className="flex-1 min-w-0 text-text-primary truncate" title={ev.device_label}>
                  {label}
                </span>
                {ev.ip && (
                  <span className="shrink-0 text-text-muted font-mono">
                    {ev.ip}
                  </span>
                )}
                <span className="shrink-0 text-text-muted">
                  {relativeTimeSec(ev.at)}
                </span>
              </div>
            )
          })}
        </div>
      )}
    </section>
  )
}
