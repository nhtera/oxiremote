import { useEffect, useState } from 'react'
import StatusChip from './ui/status-chip'
import { prettyDeviceLabel, relativeTimeMs } from '../lib/device-label'

// Active client session data from GET /api/agent/sessions/active
export interface ActiveSession {
  device_id: string
  label: string
  platform: string
  doing: 'terminal_active' | 'desktop' | 'files' | 'idle'
  last_active_ms: number
}

const POLL_MS = 5_000

function platformBadge(platform: string): string {
  const s = platform.toLowerCase()
  if (s.includes('ios') || s.includes('iphone') || s.includes('ipad')) return 'iOS'
  if (s.includes('android')) return 'Android'
  if (s.includes('mac')) return 'Mac'
  if (s.includes('win')) return 'Win'
  if (s.includes('linux')) return 'Linux'
  return '?'
}

function doingLabel(doing: ActiveSession['doing']): string {
  switch (doing) {
    case 'terminal_active': return 'Terminal'
    case 'desktop': return 'Desktop'
    case 'files': return 'Files'
    case 'idle': return 'Idle'
  }
}

// Polls /api/agent/sessions/active every 5 s. Renders a compact per-client
// row showing platform, device label, current activity chip, last-active
// relative time, and a Disconnect button. Collapses to a "no active sessions"
// hint when empty so the home page never shows a blank section.
export default function ActiveSessionsList() {
  const [sessions, setSessions] = useState<ActiveSession[]>([])
  const [error, setError] = useState(false)
  const [, setTick] = useState(0)

  useEffect(() => {
    let cancelled = false

    const poll = async () => {
      try {
        const res = await fetch('/api/agent/sessions/active')
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
        const data = (await res.json()) as ActiveSession[]
        if (!cancelled) {
          setSessions(data)
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

  // Re-render relative times every 10 s without re-fetching.
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 10_000)
    return () => window.clearInterval(id)
  }, [])

  async function disconnect(deviceId: string) {
    try {
      await fetch(`/api/agent/devices/${deviceId}/disconnect`, { method: 'POST' })
      setSessions((s) => s.filter((x) => x.device_id !== deviceId))
    } catch {
      // Non-critical — next poll will resync.
    }
  }

  return (
    <section className="rounded-xl border border-border bg-surface p-4 space-y-3">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-[length:var(--text-h3)] font-semibold text-text-primary">
          Active Sessions
        </h2>
        <span className="text-[length:var(--text-meta)] text-text-muted">
          {sessions.length} client{sessions.length !== 1 ? 's' : ''}
        </span>
      </div>

      {error && (
        <div className="text-[length:var(--text-meta)] text-danger">
          Could not load session data.
        </div>
      )}

      {!error && sessions.length === 0 && (
        <div className="text-[length:var(--text-meta)] text-text-muted">
          No active clients right now.
        </div>
      )}

      {sessions.map((s) => (
        <div
          key={s.device_id}
          className="flex items-center gap-2 border border-border rounded-lg px-3 py-2"
        >
          <span className="shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded bg-surface-alt border border-border text-text-muted min-w-8 text-center">
            {platformBadge(s.platform)}
          </span>
          <span
            className="flex-1 min-w-0 text-sm text-text-primary truncate"
            title={s.label}
          >
            {prettyDeviceLabel(s.label, s.platform)}
          </span>
          <StatusChip
            variant={s.doing === 'idle' ? 'offline' : 'online'}
            noDot={s.doing === 'idle'}
          >
            {doingLabel(s.doing)}
          </StatusChip>
          <span className="shrink-0 text-[length:var(--text-meta)] text-text-muted">
            {relativeTimeMs(s.last_active_ms)}
          </span>
          <button
            type="button"
            onClick={() => void disconnect(s.device_id)}
            className="shrink-0 text-[length:var(--text-meta)] text-danger border border-danger/30 rounded-md px-2 py-0.5 hover:bg-danger/10 transition-colors"
          >
            Disconnect
          </button>
        </div>
      ))}
    </section>
  )
}
