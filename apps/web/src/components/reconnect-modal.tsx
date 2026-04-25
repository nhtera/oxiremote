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
  /** Seconds shown in the countdown. Defaults to 3. */
  countdownSeconds?: number
}

export default function ReconnectModal({
  open,
  attempt,
  maxAttempts,
  exhausted,
  onCancel,
  onRetry,
  countdownSeconds = 3,
}: Props) {
  const [secondsLeft, setSecondsLeft] = useState(countdownSeconds)

  // Reset the visible countdown each time a new attempt begins or the modal opens.
  useEffect(() => {
    if (!open || exhausted) return
    setSecondsLeft(countdownSeconds)
    const id = window.setInterval(() => {
      setSecondsLeft((s) => (s > 0 ? s - 1 : 0))
    }, 1000)
    return () => window.clearInterval(id)
  }, [open, exhausted, attempt, countdownSeconds])

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
          `Could not reconnect after ${maxAttempts} attempts. Your terminal session is still preserved on the host — try opening the page again to resume.`
        ) : (
          <>
            Your session is preserved — we'll resume where you left off.
            <div className="text-[length:var(--text-meta)] text-text-muted mt-2">
              Retrying… ({attempt} of {maxAttempts})
              {onRetry && (
                <> — next try in <span className="font-mono">{secondsLeft}s</span></>
              )}
            </div>
          </>
        )}
      </div>
      <div className="mt-5 flex justify-end gap-2">
        {!exhausted && onRetry && (
          <Button variant="primary" size="sm" onClick={onRetry}>
            Retry now
          </Button>
        )}
        <Button variant="ghost" size="sm" onClick={onCancel}>
          {exhausted ? 'Exit' : 'Cancel'}
        </Button>
      </div>
    </Dialog>
  )
}
