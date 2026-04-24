// STUB — Phase 05 extracts full reconnect-modal with animation and history.
// Presentational only: shows attempt count, Cancel/Exit button.

interface Props {
  open: boolean
  attempt: number
  maxAttempts: number
  onCancel: () => void
  exhausted: boolean
}

export default function ReconnectModal({ open, attempt, maxAttempts, onCancel, exhausted }: Props) {
  if (!open) return null

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
    >
      <div className="w-full max-w-sm bg-surface border border-border rounded-lg p-5 shadow-xl">
        <div className="text-text-primary font-semibold text-sm mb-2">
          {exhausted ? 'Connection failed' : 'Reconnecting…'}
        </div>
        <div className="text-text-muted text-xs mb-5">
          {exhausted
            ? `Could not reconnect after ${maxAttempts} attempts.`
            : `Attempt ${attempt} of ${maxAttempts}`}
        </div>
        <div className="flex justify-end">
          <button
            onClick={onCancel}
            className="px-4 py-2 text-sm font-medium border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            {exhausted ? 'Exit' : 'Cancel'}
          </button>
        </div>
      </div>
    </div>
  )
}
