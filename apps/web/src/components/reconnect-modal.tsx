// Real reconnect modal: countdown to next attempt + manual retry + give-up.
// Built on the shared <Dialog> primitive — focus trap, Esc, backdrop, ARIA all
// inherited.
//
// Three states drive the copy:
//   - phase='fast' (default) — transient drop, ~5 attempts on a 0.5–5s
//     ladder. Countdown and progress bar visible.
//   - phase='slow' — /api/health says the host is offline (sleep/wake, agent
//     restart, network handoff). Cycles 5–30s probes; never gives up
//     automatically. Copy switches to "Host appears offline" with no
//     countdown.
//   - exhausted=true — terminal failure state used by the desktop flow,
//     where retry isn't possible without a fresh signaling channel. The
//     terminal flow never sets this; it always falls back to slow phase.

import { useEffect, useState } from 'react'
import Dialog from './ui/dialog'
import Button from './ui/button'
import type { ReconnectPhase } from '../lib/terminal-ws-hook'

interface Props {
  open: boolean
  attempt: number
  maxAttempts: number
  /** Active retry phase. 'slow' = host appears offline; we keep trying. */
  phase?: ReconnectPhase
  /** Terminal-failure flag for callers that DO have a give-up state (the
   *  desktop session uses it). Terminal sessions never set this. */
  exhausted?: boolean
  /** Tear down the session — used as the give-up / cancel action. */
  onCancel: () => void
  /** Optional: trigger a manual retry now. When unset, only Cancel is shown. */
  onRetry?: () => void
  /** Real fast-phase backoff (ms) the hook will wait before the next attempt.
   *  Drives both the seconds label and the progress bar in fast phase. */
  countdownMs?: number
}

export default function ReconnectModal({
  open,
  attempt,
  maxAttempts,
  phase = 'fast',
  exhausted = false,
  onCancel,
  onRetry,
  countdownMs = 3000,
}: Props) {
  const isSlow = phase === 'slow' && !exhausted
  const isFast = !isSlow && !exhausted
  const totalMs = Math.max(countdownMs, 250)
  const [msLeft, setMsLeft] = useState(totalMs)

  // Reset the fast-phase countdown each time a new attempt begins or the
  // modal opens. We tick at 100ms for a smooth bar; the label rounds to
  // whole seconds. Skip in slow / exhausted — countdown is meaningless.
  useEffect(() => {
    if (!open || !isFast) return
    setMsLeft(totalMs)
    const tickMs = 100
    const id = window.setInterval(() => {
      setMsLeft((ms) => (ms > tickMs ? ms - tickMs : 0))
    }, tickMs)
    return () => window.clearInterval(id)
  }, [open, isFast, attempt, totalMs])

  const secondsLeft = Math.ceil(msLeft / 1000)
  // Bar reflects how far we are through the fast retry budget. Hidden in
  // slow / exhausted — attempts either cycle or are over.
  const pct = Math.min(100, Math.round((attempt / Math.max(maxAttempts, 1)) * 100))

  return (
    <Dialog
      open={open}
      onClose={onCancel}
      dismissable={exhausted}
      tone={isFast ? 'warning' : 'danger'}
      ariaLabelledBy="reconnect-title"
      ariaDescribedBy="reconnect-desc"
    >
      <div id="reconnect-title" className="text-text-primary font-semibold text-[length:var(--text-h2)]">
        {exhausted ? 'Connection failed' : isSlow ? 'Host appears offline' : 'Connection lost'}
      </div>
      <div id="reconnect-desc" className="text-text-secondary text-[length:var(--text-body)] mt-2 leading-relaxed">
        {exhausted ? (
          `Could not reconnect after ${maxAttempts} attempts. Your session is still preserved on the host — try opening the page again to resume.`
        ) : isSlow ? (
          `We'll keep trying — your session is preserved on the host. This usually clears within a minute of the host waking up.`
        ) : (
          <>
            Your session is preserved — we'll resume where you left off.
            {onRetry && (
              <div className="text-[length:var(--text-meta)] text-text-muted mt-2">
                Reconnecting… Attempt {attempt} of {maxAttempts} — next try in{' '}
                <span className="font-mono">{secondsLeft}s</span>
              </div>
            )}
          </>
        )}
      </div>

      {isFast && (
        <div className="mt-4 h-1.5 w-full rounded-full bg-surface-alt overflow-hidden">
          <div
            className="h-full bg-orange-500 transition-[width] duration-300 ease-out"
            style={{ width: `${pct}%` }}
            role="progressbar"
            aria-valuenow={attempt}
            aria-valuemin={0}
            aria-valuemax={maxAttempts}
            aria-label={`Reconnect attempt ${attempt} of ${maxAttempts}`}
          />
        </div>
      )}

      <div className="mt-4 flex justify-end gap-2">
        {!exhausted && onRetry && (
          <Button variant="primary" size="sm" onClick={onRetry}>
            Retry now
          </Button>
        )}
        <Button variant={exhausted ? 'danger' : 'ghost'} size="sm" onClick={onCancel}>
          Exit
        </Button>
      </div>
    </Dialog>
  )
}
