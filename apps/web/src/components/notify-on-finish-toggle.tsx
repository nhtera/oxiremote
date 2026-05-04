// Toggle shown in the terminal tab context menu when an agent CLI is detected.
// POSTs to /api/terminal/sessions/:id/notify-on-finish to persist the opt-in.
// Displayed only when session.detected_agent is set.

import { useState } from 'react'

type Props = {
  sessionId: string
  agentName: string
  enabled: boolean
  onToggled: (enabled: boolean) => void
}

export default function NotifyOnFinishToggle({ sessionId, agentName, enabled, onToggled }: Props) {
  const [pending, setPending] = useState(false)

  async function handleClick() {
    if (pending) return
    const next = !enabled
    setPending(true)
    try {
      const res = await fetch(`/api/terminal/sessions/${sessionId}/notify-on-finish`, {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: next }),
      })
      if (res.ok) {
        onToggled(next)
      }
    } catch {
      // Silently ignore network errors — toggle reverts to prior state.
    } finally {
      setPending(false)
    }
  }

  return (
    <button
      onClick={handleClick}
      disabled={pending}
      className="flex items-center gap-2 w-full px-3 py-2 text-left text-xs text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors disabled:opacity-50"
      title="Last 5 lines of output may be included in the notification and could contain secrets."
    >
      {/* Inline checkbox indicator */}
      <span
        className={`w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 ${
          enabled
            ? 'bg-accent border-accent text-white'
            : 'border-border bg-surface'
        }`}
        aria-hidden="true"
      >
        {enabled && (
          <svg viewBox="0 0 10 10" className="w-2 h-2" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
            <polyline points="1.5 5 4 7.5 8.5 2" />
          </svg>
        )}
      </span>
      <span>
        Notify when <span className="text-text-primary font-medium">{agentName}</span> finishes
      </span>
    </button>
  )
}
