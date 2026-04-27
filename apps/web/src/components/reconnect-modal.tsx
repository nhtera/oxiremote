// Real reconnect modal: countdown to next attempt + manual retry + give-up.
// Built on the shared <Dialog> primitive — focus trap, Esc, backdrop, ARIA all
// inherited. The countdown is presentational: actual reconnect cadence lives
// in the session hook; this only paints the seconds remaining.

import { useEffect, useState } from 'react'
import Dialog from './ui/dialog'
import Button from './ui/button'

interface Props {
  open: boolean
  attempt: number
  maxAttempts: number
  exhausted: boolean
  /** Tear down the session — used as the give-up / cancel action. */
  onCancel: () => void
  /** Optional: trigger a manual retry now. When unset, only Cancel is shown. */
  onRetry?: () => void
  /** Real backoff (ms) the hook will wait before the next attempt. Drives both
   *  the seconds label and the progress bar so the UI matches reality. */
  countdownMs?: number
}

export default function ReconnectModal({
  open,
  attempt,
  maxAttempts,
  exhausted,
  onCancel,
  onRetry,
  countdownMs = 3000,
}: Props) {
  const totalMs = Math.max(countdownMs, 250)
  const [msLeft, setMsLeft] = useState(totalMs)

  // Reset the countdown each time a new attempt begins or the modal opens. We
  // tick at 100ms for a smooth bar; the label rounds to whole seconds.
  useEffect(() => {
    if (!open || exhausted) return
    setMsLeft(totalMs)
    const tickMs = 100
    const id = window.setInterval(() => {
      setMsLeft((ms) => (ms > tickMs ? ms - tickMs : 0))
    }, tickMs)
    return () => window.clearInterval(id)
  }, [open, exhausted, attempt, totalMs])

  const secondsLeft = Math.ceil(msLeft / 1000)
  // Bar reflects how far we are through the retry budget (attempt N of M),
  // not the per-attempt countdown — the user wants a sense of "running out
  // of tries", not "next tick is X% away".
  const pct = Math.min(100, Math.round((attempt / Math.max(maxAttempts, 1)) * 100))

  return (
    <Dialog
      open={open}
      onClose={onCancel}
      dismissable={exhausted}
      tone={exhausted ? 'danger' : 'warning'}
      ariaLabelledBy="reconnect-title"
      ariaDescribedBy="reconnect-desc"
    >
      <div id="reconnect-title" className="text-text-primary font-semibold text-[length:var(--text-h2)]">
        {exhausted ? 'Connection failed' : 'Connection lost'}
      </div>
      <div id="reconnect-desc" className="text-text-secondary text-[length:var(--text-body)] mt-2 leading-relaxed">
        {exhausted ? (
          `Could not reconnect after ${maxAttempts} attempts. Your session is still preserved on the host — try opening the page again to resume.`
        ) : (
          <>
            Your session is preserved — we'll resume where you left off.
            <div className="text-[length:var(--text-meta)] text-text-muted mt-2">
              Reconnecting… Attempt {attempt} of {maxAttempts}
              {onRetry && (
                <> — next try in <span className="font-mono">{secondsLeft}s</span></>
              )}
            </div>
          </>
        )}
      </div>

      {/* Orange progress bar — visible only while retrying. Width reflects
          attempts used out of the retry budget (e.g. 5/8 → 62%). */}
      {!exhausted && (
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
