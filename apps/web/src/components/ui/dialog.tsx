import { useEffect, useRef, type ReactNode } from 'react'

export type DialogTone = 'default' | 'warning' | 'danger'

interface Props {
  open: boolean
  onClose?: () => void
  /** When false, clicking the backdrop or pressing Esc does nothing. */
  dismissable?: boolean
  ariaLabelledBy?: string
  ariaDescribedBy?: string
  children: ReactNode
  /** Extra classes for the panel (size, padding, etc.). */
  panelClassName?: string
  /** Border tint conveying intent. Reserve warning/danger for irreversible actions. */
  tone?: DialogTone
}

const TONE_BORDER: Record<DialogTone, string> = {
  default: 'border-border',
  warning: 'border-warning/40',
  danger: 'border-danger/40',
}

const FOCUSABLE_SELECTOR =
  'a[href], area[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), ' +
  'button:not([disabled]), iframe, [tabindex]:not([tabindex="-1"]), [contenteditable="true"]'

// Base modal: focus trap + Esc + backdrop. ~70 LOC by design — does NOT include
// transitions/animation; primitives that need motion (toast) layer it on top.
export default function Dialog({
  open,
  onClose,
  dismissable = true,
  ariaLabelledBy,
  ariaDescribedBy,
  children,
  panelClassName,
  tone = 'default',
}: Props) {
  const panelRef = useRef<HTMLDivElement>(null)
  const previouslyFocusedRef = useRef<HTMLElement | null>(null)

  // Focus management: capture caller's focus, restore on close.
  useEffect(() => {
    if (!open) return
    previouslyFocusedRef.current = document.activeElement as HTMLElement | null

    // Defer focus to next tick so the panel has rendered.
    const t = window.setTimeout(() => {
      const panel = panelRef.current
      if (!panel) return
      const first = panel.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)
      ;(first ?? panel).focus()
    }, 0)

    return () => {
      window.clearTimeout(t)
      // Restore focus to the trigger when the dialog closes.
      const prev = previouslyFocusedRef.current
      if (prev && document.contains(prev)) prev.focus()
    }
  }, [open])

  // Esc + Tab cycling
  useEffect(() => {
    if (!open) return

    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape' && dismissable && onClose) {
        e.stopPropagation()
        onClose()
        return
      }
      if (e.key !== 'Tab') return
      const panel = panelRef.current
      if (!panel) return
      const focusables = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
      if (focusables.length === 0) {
        e.preventDefault()
        panel.focus()
        return
      }
      const first = focusables[0]
      const last = focusables[focusables.length - 1]
      const active = document.activeElement
      if (e.shiftKey && active === first) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && active === last) {
        e.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, dismissable, onClose])

  // Lock body scroll while open.
  useEffect(() => {
    if (!open) return
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = prev
    }
  }, [open])

  if (!open) return null

  function onBackdropClick() {
    if (dismissable && onClose) onClose()
  }

  const panelCls =
    `relative z-10 w-full max-w-sm bg-surface border ${TONE_BORDER[tone]} rounded-lg shadow-xl outline-none ` +
    (panelClassName ?? 'p-5')

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div
        className="absolute inset-0 bg-black/60"
        onClick={onBackdropClick}
        aria-hidden="true"
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={ariaLabelledBy}
        aria-describedby={ariaDescribedBy}
        tabIndex={-1}
        className={panelCls}
      >
        {children}
      </div>
    </div>
  )
}
