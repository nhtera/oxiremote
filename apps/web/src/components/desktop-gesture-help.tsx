// Gesture help bottom sheet — two tabs: Touch and Trackpad.
// Dismisses on backdrop click or X button. Orange underline on active tab.

import { useState } from 'react'

interface Props {
  open: boolean
  onClose: () => void
}

type Tab = 'touch' | 'trackpad'

const TOUCH_GESTURES = [
  { gesture: 'Tap', action: 'Left click' },
  { gesture: 'Two-finger tap', action: 'Right click' },
  { gesture: 'Long press (0.5s)', action: 'Right click' },
  { gesture: 'Two-finger swipe', action: 'Scroll' },
  { gesture: 'Drag', action: 'Click and drag' },
  { gesture: 'Three-finger swipe', action: 'Reserved (v2)' },
  { gesture: 'Pinch', action: 'Zoom (disabled v1)' },
]

export default function DesktopGestureHelp({ open, onClose }: Props) {
  const [tab, setTab] = useState<Tab>('touch')

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-40 flex items-end justify-center"
      onClick={onClose}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/50" />

      {/* Sheet */}
      <div
        className="relative z-10 w-full max-w-lg bg-surface border border-border rounded-t-xl p-4 pb-safe"
        onClick={(e) => e.stopPropagation()}
      >
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
                  ? 'text-orange-400 border-b-2 border-orange-500 -mb-px font-medium'
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
          <div className="grid gap-1.5 text-xs">
            <div className="flex justify-between gap-3">
              <span className="text-text-muted">Move</span>
              <span className="text-text-primary text-right">Move cursor</span>
            </div>
            <div className="flex justify-between gap-3">
              <span className="text-text-muted">Click</span>
              <span className="text-text-primary text-right">Left click</span>
            </div>
            <div className="flex justify-between gap-3">
              <span className="text-text-muted">Right-click</span>
              <span className="text-text-primary text-right">Right click</span>
            </div>
            <div className="flex justify-between gap-3">
              <span className="text-text-muted">Two-finger scroll</span>
              <span className="text-text-primary text-right">Scroll</span>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
