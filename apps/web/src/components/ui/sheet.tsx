import { useEffect, useRef, type ReactNode } from 'react'

type Side = 'right' | 'left' | 'bottom'

interface Props {
  open: boolean
  onClose?: () => void
  /** Where the sheet slides in from. Default 'right'. */
  side?: Side
  /** Tailwind width/height tokens for the panel. Default sane per side. */
  panelClassName?: string
  ariaLabelledBy?: string
  ariaDescribedBy?: string
  children: ReactNode
}

const FOCUSABLE_SELECTOR =
  'a[href], area[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), ' +
  'button:not([disabled]), iframe, [tabindex]:not([tabindex="-1"]), [contenteditable="true"]'

// Side-anchored sheet/drawer. Same focus-trap + Esc + body-scroll lock as
// Dialog, but the panel is full-height (or full-width for bottom) and
// anchored to a viewport edge. Used by the gear drawer and the
// Files/Git/Preview compact-mode wrappers.
export default function Sheet({
  open, onClose, side = 'right', panelClassName,
  ariaLabelledBy, ariaDescribedBy, children,
}: Props) {
  const panelRef = useRef<HTMLDivElement>(null)
  const previouslyFocusedRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    if (!open) return
    previouslyFocusedRef.current = document.activeElement as HTMLElement | null
    const t = window.setTimeout(() => {
      const panel = panelRef.current
      if (!panel) return
      const first = panel.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)
      ;(first ?? panel).focus()
    }, 0)
    return () => {
      window.clearTimeout(t)
      const prev = previouslyFocusedRef.current
      if (prev && document.contains(prev)) prev.focus()
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape' && onClose) {
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
      if (e.shiftKey && active === first) { e.preventDefault(); last.focus() }
      else if (!e.shiftKey && active === last) { e.preventDefault(); first.focus() }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, onClose])

  useEffect(() => {
    if (!open) return
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => { document.body.style.overflow = prev }
  }, [open])

  if (!open) return null

  // Panel position + default sizing per side. Mobile gets full-screen on
  // right/left so the small viewport isn't cut in half.
  const sideCls =
    side === 'right' ? 'right-0 top-0 h-full w-full sm:w-96 max-w-full' :
    side === 'left' ? 'left-0 top-0 h-full w-full sm:w-96 max-w-full' :
    'left-0 right-0 bottom-0 w-full max-h-[80vh]'

  return (
    <div className="fixed inset-0 z-50">
      <div
        className="absolute inset-0 bg-black/60"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={ariaLabelledBy}
        aria-describedby={ariaDescribedBy}
        tabIndex={-1}
        className={`absolute ${sideCls} bg-surface border-border ${
          side === 'right' ? 'border-l' : side === 'left' ? 'border-r' : 'border-t'
        } shadow-2xl outline-none overflow-y-auto ${panelClassName ?? ''}`}
        style={
          side === 'bottom'
            ? { paddingBottom: 'env(safe-area-inset-bottom)', paddingLeft: 'env(safe-area-inset-left)', paddingRight: 'env(safe-area-inset-right)' }
            : { paddingTop: 'env(safe-area-inset-top)', paddingBottom: 'env(safe-area-inset-bottom)' }
        }
      >
        {children}
      </div>
    </div>
  )
}
