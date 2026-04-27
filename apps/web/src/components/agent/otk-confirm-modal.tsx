import { useEffect } from 'react'

interface Props {
  onConfirm: () => void
  onCancel: () => void
}

// Confirm dialog before invalidating the current OTK. Same overlay style as
// ApprovalModal so the two feel like the same system.
export default function OtkConfirmModal({ onConfirm, onCancel }: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm p-4"
      onClick={onCancel}
    >
      <div
        className="w-full max-w-sm bg-surface border border-border rounded-lg p-5 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="text-text-primary font-semibold text-sm mb-2">
          Generate a new one-time key?
        </div>
        <p className="text-xs text-text-secondary leading-relaxed mb-5">
          The current QR will stop working immediately. A device scanning right
          now will need the new key.
        </p>
        <div className="flex gap-2 justify-end">
          <button
            onClick={onCancel}
            className="px-4 py-2 text-sm font-medium border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="px-4 py-2 text-sm font-medium bg-accent/20 text-accent border border-accent/40 rounded-md hover:bg-accent/30 transition-colors"
          >
            Generate
          </button>
        </div>
      </div>
    </div>
  )
}
