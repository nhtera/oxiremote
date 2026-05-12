// Gesture help bottom sheet — two tabs: Touch and Trackpad.
// Dismisses on backdrop click, X button, or Escape key.
// Accent underline on active tab.
// Content covers all gestures from the UX parity spec plus toolbar legend.

import { useEffect, useState } from 'react'

interface Props {
  open: boolean
  onClose: () => void
}

type Tab = 'touch' | 'trackpad'

const TOUCH_GESTURES = [
  { gesture: '1-finger tap', action: 'Left click' },
  { gesture: 'Double tap', action: 'Double click' },
  { gesture: '1-finger drag (no zoom)', action: 'Click and drag' },
  { gesture: 'Long press (0.5 s)', action: 'Right click' },
  { gesture: '2-finger tap', action: 'Right click' },
  { gesture: '3-finger tap', action: 'Middle click' },
  { gesture: '2-finger swipe', action: 'Scroll' },
  { gesture: '2-finger pinch', action: 'Zoom canvas' },
  { gesture: '1-finger drag (zoomed in)', action: 'Pan view' },
  { gesture: 'Rectangle mode (▢) + drag', action: 'Marquee select on remote' },
]

const TRACKPAD_GESTURES = [
  { gesture: 'Move', action: 'Move cursor' },
  { gesture: 'Tap', action: 'Left click' },
  { gesture: 'Long press (0.5 s)', action: 'Right click' },
  { gesture: '2-finger swipe', action: 'Scroll' },
  { gesture: '2-finger pinch', action: 'Zoom canvas' },
  { gesture: 'Edge of viewport (zoomed)', action: 'Auto-pan to follow cursor' },
]

const TOOLBAR_LEGEND = [
  { key: 'Touch / Trackpad', desc: 'Toggle pointer input mode' },
  { key: 'Aa', desc: 'Open text-batch input for long strings' },
  { key: '▢', desc: 'Rectangle-select mode (drag to draw a marquee)' },
  { key: '?', desc: 'Show this gesture guide' },
]

export default function DesktopGestureHelp({ open, onClose }: Props) {
  const [tab, setTab] = useState<Tab>('touch')

  // Esc closes the sheet
  useEffect(() => {
    if (!open) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, onClose])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-40 flex items-end justify-center"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label="Gesture guide"
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/50" aria-hidden="true" />

      {/* Sheet */}
      <div
        className="relative z-10 w-full max-w-lg bg-surface border border-border rounded-t-xl p-4"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 1rem)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Drag handle */}
        <div className="mx-auto mb-3 h-1 w-10 rounded-full bg-border" />

        {/* Header */}
        <div className="flex items-center justify-between mb-3">
          <span className="text-sm font-medium text-text-primary">Gesture guide</span>
          <button
            onClick={onClose}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded"
            aria-label="Close gesture help"
          >
            ✕
          </button>
        </div>

        {/* Tabs */}
        <div className="flex gap-4 border-b border-border mb-3">
          {(['touch', 'trackpad'] as Tab[]).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={[
                'pb-2 text-sm capitalize transition-colors',
                tab === t
                  ? 'text-[hsl(var(--accent-primary))] border-b-2 border-[hsl(var(--accent-primary))] -mb-px font-medium'
                  : 'text-text-muted hover:text-text-secondary',
              ].join(' ')}
            >
              {t}
            </button>
          ))}
        </div>

        {/* Tab content */}
        {tab === 'touch' ? (
          <div className="grid gap-1.5">
            {TOUCH_GESTURES.map(({ gesture, action }) => (
              <div key={gesture} className="flex justify-between text-xs gap-3">
                <span className="text-text-muted shrink-0">{gesture}</span>
                <span className="text-text-primary text-right">{action}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="grid gap-1.5">
            {TRACKPAD_GESTURES.map(({ gesture, action }) => (
              <div key={gesture} className="flex justify-between text-xs gap-3">
                <span className="text-text-muted shrink-0">{gesture}</span>
                <span className="text-text-primary text-right">{action}</span>
              </div>
            ))}
          </div>
        )}

        {/* Toolbar legend */}
        <div className="mt-4 pt-3 border-t border-border">
          <div className="text-xs font-medium text-text-secondary mb-2">Toolbar</div>
          <div className="grid gap-1.5">
            {TOOLBAR_LEGEND.map(({ key, desc }) => (
              <div key={key} className="flex justify-between text-xs gap-3">
                <span className="text-text-muted shrink-0 font-mono">{key}</span>
                <span className="text-text-primary text-right">{desc}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
