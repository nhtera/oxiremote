import { useCallback, useEffect, useState } from 'react'

// Localhost-only list of paired devices. /api/agent/devices is auth-free on
// loopback (the host operator owns the machine), so this works on a freshly
// booted agent that hasn't paired anything yet.

interface TrustedDevice {
  device_id: string
  label: string
  user_agent: string | null
  created_at: number
  last_seen_at: number
  revoked_at: number | null
}

const POLL_MS = 10_000

function relTime(tsSec: number): string {
  if (!tsSec) return '—'
  const diff = Math.floor(Date.now() / 1000 - tsSec)
  if (diff < 5) return 'just now'
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86_400)}d ago`
}

export default function DevicesPanel() {
  const [devices, setDevices] = useState<TrustedDevice[]>([])

  const refresh = useCallback(async () => {
    try {
      const res = await fetch('/api/agent/devices')
      if (!res.ok) return
      setDevices((await res.json()) as TrustedDevice[])
    } catch {
      // network blip; next poll recovers
    }
  }, [])

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, POLL_MS)
    return () => clearInterval(t)
  }, [refresh])

  if (devices.length === 0) {
    return (
      <div className="text-xs text-text-muted">
        No paired devices yet. Scan the QR or share the one-time key.
      </div>
    )
  }

  return (
    <div className="grid gap-1">
      {devices.map((d) => (
        <div
          key={d.device_id}
          className="flex items-center gap-2 py-1.5 border-b border-border/60 last:border-0"
        >
          <span className="inline-block w-2 h-2 rounded-full bg-success" />
          <span className="flex-1 truncate text-sm text-text-primary" title={d.label}>
            {d.label}
          </span>
          <span
            className="text-xs text-text-muted"
            title={new Date(d.last_seen_at * 1000).toLocaleString()}
          >
            {relTime(d.last_seen_at)}
          </span>
        </div>
      ))}
    </div>
  )
}
