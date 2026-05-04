import { useCallback, useEffect, useState } from 'react'
import { WifiIcon } from './icons'
import { prettyDeviceLabel, relativeTimeSec } from '../lib/device-label'

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
  /** User-supplied display name. Takes precedence over the UA-derived label. */
  device_name?: string | null
  /** Coarse OS hint captured at pair time — fallback when UA parse misses. */
  platform?: string | null
}

const POLL_MS = 10_000
const STALE_AFTER_SEC = 300 // 5 min — treat as offline/grey

function statusFor(d: TrustedDevice): { dotClass: string; title: string } {
  if (d.revoked_at) return { dotClass: 'bg-danger', title: 'Revoked' }
  const idleSec = Math.floor(Date.now() / 1000 - d.last_seen_at)
  if (idleSec > STALE_AFTER_SEC) return { dotClass: 'bg-text-muted', title: 'Offline' }
  return { dotClass: 'bg-success', title: 'Online' }
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
      <div className="flex flex-col items-center justify-center text-center py-6 px-4 gap-2">
        <span className="inline-flex w-10 h-10 items-center justify-center rounded-full bg-accent/10 border border-accent/30 text-accent">
          <WifiIcon size={20} />
        </span>
        <div className="text-sm font-medium text-text-primary">No clients yet</div>
        <div className="text-xs text-text-muted leading-relaxed max-w-[280px]">
          Scan the QR code to connect your first device.
        </div>
      </div>
    )
  }

  return (
    <div className="grid gap-1">
      {devices.map((d) => {
        const { dotClass, title } = statusFor(d)
        // Prefer user-set name, then friendly UA cue ("Mac · Chrome"), then
        // raw label as last resort. Full UA stays in the title attribute so
        // power users can hover for forensic detail.
        const display = d.device_name?.trim()
          || prettyDeviceLabel(d.label, d.platform ?? undefined)
        const fullUa = d.user_agent || d.label
        return (
          <div
            key={d.device_id}
            className="flex items-center gap-2.5 py-1.5 border-b border-border/60 last:border-0"
          >
            <span className={`inline-block w-2.5 h-2.5 rounded-full ${dotClass} shrink-0`} title={title} />
            <span className="text-text-muted shrink-0" aria-hidden>
              <WifiIcon size={14} />
            </span>
            <span className="flex-1 truncate text-sm text-text-primary" title={fullUa}>
              {display}
            </span>
            <span
              className="text-xs text-text-muted"
              title={new Date(d.last_seen_at * 1000).toLocaleString()}
            >
              {relativeTimeSec(d.last_seen_at)}
            </span>
          </div>
        )
      })}
    </div>
  )
}
