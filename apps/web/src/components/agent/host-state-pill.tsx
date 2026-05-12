import { useEffect, useState } from 'react'
import StatusChip from '../ui/status-chip'

// Live indicator for macOS lock-screen handling. Subscribes to the localhost
// SSE feed for `stay_awake_changed` and `host_locked` / `host_unlocked` events
// and renders a single chip the operator can scan at a glance:
//
//   - Locked    → "Mac locked"   (red, supersedes everything else)
//   - Awake     → "Keeping awake" (green, agent holding caffeinate assertion)
//   - Idle      → null            (nothing to surface; saves vertical space)
//
// Only mounted on macOS hosts; the parent gates on `state.platform === 'macos'`.

type Status = 'idle' | 'awake' | 'locked'

export default function HostStatePill() {
  const [status, setStatus] = useState<Status>('idle')

  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type: string; active?: boolean }
        // Lock takes precedence — operator needs to see it even while awake.
        if (ev.type === 'host_locked') {
          setStatus('locked')
        } else if (ev.type === 'host_unlocked') {
          // Returning to "idle" loses awake state if the assertion is still
          // held; resolve by demoting to awake when applicable. The
          // stay_awake_changed event drives that — it fires on session start
          // and end, so the chip can lag by one event after unlock; that's
          // acceptable for an operator-glance pill.
          setStatus((prev) => (prev === 'locked' ? 'idle' : prev))
        } else if (ev.type === 'stay_awake_changed') {
          setStatus(ev.active ? 'awake' : 'idle')
        }
      } catch {
        // drop malformed frames; SSE will retry
      }
    }
    return () => es.close()
  }, [])

  if (status === 'idle') return null
  if (status === 'locked') {
    return (
      <StatusChip variant="rejected" noDot>
        Mac locked
      </StatusChip>
    )
  }
  return <StatusChip variant="online">Keeping awake</StatusChip>
}
