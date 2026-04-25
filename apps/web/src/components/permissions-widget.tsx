import { useCallback, useEffect, useState } from 'react'

// macOS TCC + Accessibility status. Polls /api/agent/permissions every 5s
// (drops to 1s for 30s after a grant click — captures the change as soon as
// the operator flips the toggle in System Settings). On non-macOS platforms
// the backend reports both `true` so the widget collapses to a "no setup
// required" hint.

interface PermsResponse {
  screen_recording: boolean
  accessibility: boolean
  platform: string
  supported: boolean
}

const SLOW_POLL_MS = 5_000
const FAST_POLL_MS = 1_000
const FAST_POLL_DURATION_MS = 30_000

export default function PermissionsWidget() {
  const [perms, setPerms] = useState<PermsResponse | null>(null)
  const [fastUntil, setFastUntil] = useState<number>(0)

  const refresh = useCallback(async () => {
    try {
      const res = await fetch('/api/agent/permissions')
      if (!res.ok) return
      setPerms((await res.json()) as PermsResponse)
    } catch {
      // network blip; next poll recovers
    }
  }, [])

  useEffect(() => {
    refresh()
    const tick = () => {
      refresh()
      const interval = Date.now() < fastUntil ? FAST_POLL_MS : SLOW_POLL_MS
      handle = setTimeout(tick, interval)
    }
    let handle = setTimeout(tick, SLOW_POLL_MS)
    return () => clearTimeout(handle)
  }, [refresh, fastUntil])

  const grant = async (kind: 'screen_recording' | 'accessibility') => {
    setFastUntil(Date.now() + FAST_POLL_DURATION_MS)
    try {
      await fetch('/api/agent/permissions/grant', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind }),
      })
    } catch {
      // open() failure is logged server-side; UI just resumes polling
    }
  }

  if (!perms) {
    return <div className="text-xs text-text-muted">Probing…</div>
  }

  if (perms.platform !== 'macos') {
    return (
      <div className="text-xs text-text-muted">
        No setup required on {perms.platform}.
      </div>
    )
  }

  return (
    <div className="space-y-2">
      <Row
        label="Screen Recording"
        granted={perms.screen_recording}
        onGrant={() => grant('screen_recording')}
      />
      <Row
        label="Accessibility (input)"
        granted={perms.accessibility}
        onGrant={() => grant('accessibility')}
      />
      <p className="text-[11px] text-text-muted pt-1">
        Toggling in System Settings reflects here within ~1s.
      </p>
    </div>
  )
}

function Row({
  label,
  granted,
  onGrant,
}: {
  label: string
  granted: boolean
  onGrant: () => void
}) {
  return (
    <div className="flex items-center gap-2 text-sm">
      <span
        className={`inline-block w-2 h-2 rounded-full ${
          granted ? 'bg-success' : 'bg-warning'
        }`}
        aria-label={granted ? 'granted' : 'not granted'}
      />
      <span className="flex-1 text-text-primary">{label}</span>
      <span
        className={`text-xs ${granted ? 'text-success' : 'text-warning'}`}
      >
        {granted ? 'granted' : 'not granted'}
      </span>
      {!granted && (
        <button
          type="button"
          onClick={onGrant}
          className="btn-secondary text-xs py-0.5 px-2"
        >
          Open Settings
        </button>
      )}
    </div>
  )
}
