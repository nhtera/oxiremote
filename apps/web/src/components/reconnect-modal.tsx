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
      ariaLabelledBy="reconnect-title"
      ariaDescribedBy="reconnect-desc"
    >
      <div id="reconnect-title" className="text-text-primary font-semibold text-sm">
        {exhausted ? 'Connection failed' : 'Reconnecting…'}
      </div>
      <div id="reconnect-desc" className="text-text-muted text-xs mt-2">
        {exhausted
          ? `Could not reconnect after ${maxAttempts} attempts.`
          : (
            <>
              Attempt {attempt} of {maxAttempts}
              {!exhausted && onRetry && (
                <> — next try in <span className="font-mono">{secondsLeft}s</span></>
              )}
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
          {exhausted ? 'Exit' : 'Give up'}
        </Button>
      </div>
    </Dialog>
  )
}
