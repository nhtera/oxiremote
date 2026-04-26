// Exit-confirm modal for mobile remote desktop.
// Shown when the user presses the hardware/browser back button during an
// active session. PC path skips this — no native back gesture on desktop.
// Built on the shared Dialog primitive: focus-trap, Esc, backdrop, ARIA.

import Dialog from './ui/dialog'
import Button from './ui/button'

interface Props {
  open: boolean
  onCancel: () => void
  onExit: () => void
}

export default function DesktopExitConfirm({ open, onCancel, onExit }: Props) {
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      dismissable
      tone="danger"
      ariaLabelledBy="exit-confirm-title"
      ariaDescribedBy="exit-confirm-desc"
    >
      <div
        id="exit-confirm-title"
        className="text-text-primary font-semibold text-[length:var(--text-h2)]"
      >
        Exit remote desktop
      </div>
      <div
        id="exit-confirm-desc"
        className="text-text-secondary text-[length:var(--text-body)] mt-2 leading-relaxed"
      >
        Are you sure you want to exit the current remote desktop session?
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="danger" size="sm" onClick={onExit}>
          Exit
        </Button>
      </div>
    </Dialog>
  )
}
