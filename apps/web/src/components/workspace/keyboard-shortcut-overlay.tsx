// Keyboard shortcut reference overlay for the workspace.
// Toggled by pressing ? (when xterm is not focused).
// Dismissed via ?, Esc, or clicking the backdrop.

import { useEffect } from 'react'
import { WORKSPACE_SHORTCUTS } from './workspace-shortcuts-data'

interface Props {
  open: boolean
  onClose: () => void
}

export default function KeyboardShortcutOverlay({ open, onClose }: Props) {
  // Dismiss on Esc or ? keydown.
  useEffect(() => {
    if (!open) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape' || e.key === '?') {
        e.preventDefault()
        onClose()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, onClose])

  if (!open) return null

  const terminal = WORKSPACE_SHORTCUTS.filter((s) => s.category === 'terminal')
  const workspace = WORKSPACE_SHORTCUTS.filter((s) => s.category === 'workspace')

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-labelledby="shortcut-overlay-title"
      onClick={onClose}
    >
      {/* Stop click propagation so clicking inside the panel doesn't close it. */}
      <div
        className="bg-surface border border-border rounded-xl shadow-2xl w-full max-w-2xl mx-4 p-5 max-h-[80vh] overflow-y-auto"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-4">
          <h2 id="shortcut-overlay-title" className="text-text-primary font-semibold text-[length:var(--text-h2)]">
            Keyboard shortcuts
          </h2>
          <button
            className="text-text-muted hover:text-text-primary transition-colors text-lg leading-none"
            onClick={onClose}
            aria-label="Close"
          >
            ✕
          </button>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <ShortcutSection title="Terminal" shortcuts={terminal} />
          <ShortcutSection title="Workspace" shortcuts={workspace} />
        </div>

        <p className="text-[length:var(--text-meta)] text-text-muted mt-4 text-center">
          Press <kbd className="px-1 py-0.5 rounded bg-surface-alt border border-border font-mono text-[10px]">?</kbd> or <kbd className="px-1 py-0.5 rounded bg-surface-alt border border-border font-mono text-[10px]">Esc</kbd> to close
        </p>
      </div>
    </div>
  )
}

function ShortcutSection({
  title,
  shortcuts,
}: {
  title: string
  shortcuts: { key: string; action: string }[]
}) {
  return (
    <div>
      <h3 className="text-[length:var(--text-meta)] font-medium uppercase tracking-wide text-text-muted mb-2">
        {title}
      </h3>
      <dl className="space-y-1.5">
        {shortcuts.map((s) => (
          <div key={s.key} className="flex items-start gap-3">
            <dt className="shrink-0">
              <kbd className="inline-block px-1.5 py-0.5 rounded bg-surface-alt border border-border font-mono text-[length:var(--text-meta)] text-text-secondary whitespace-nowrap">
                {s.key}
              </kbd>
            </dt>
            <dd className="text-[length:var(--text-body)] text-text-secondary leading-tight pt-0.5">
              {s.action}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  )
}
