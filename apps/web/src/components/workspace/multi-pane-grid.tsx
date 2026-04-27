import { useEffect, useState } from 'react'
import type { TerminalPrefs } from '../../lib/terminal-prefs'
import type { PaneAssignments, PaneCount, PaneIndex } from '../../state/terminal-store'
import XtermPane from './xterm-pane'

type Props = {
  paneCount: PaneCount
  paneAssignments: PaneAssignments
  focusedPane: PaneIndex
  prefs: TerminalPrefs
  reconnectNonce: number
  onFocusPane: (idx: PaneIndex) => void
  onConnectedChange: (sessionId: string, connected: boolean) => void
  onReconnectAttempt: (sessionId: string, attempt: number) => void
  onReconnectExhausted: (sessionId: string) => void
  onError: (msg: string) => void
  registerSend: (sessionId: string, sendFn: ((data: string) => void) | null) => void
}

export default function MultiPaneGrid({
  paneCount, paneAssignments, focusedPane, prefs, reconnectNonce,
  onFocusPane, onConnectedChange, onReconnectAttempt, onReconnectExhausted, onError, registerSend,
}: Props) {
  // Each pane needs ~280px to feel like a real terminal; below that the user
  // is better off with a single pane. We watch the viewport so /workspace on
  // mobile collapses to 1 pane regardless of the user's last desktop choice.
  const [isNarrow, setIsNarrow] = useState(() => window.matchMedia('(max-width: 768px)').matches)
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 768px)')
    const onChange = () => setIsNarrow(mq.matches)
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [])
  const effectiveCount: PaneCount = isNarrow ? 1 : paneCount

  const indices: PaneIndex[] = Array.from({ length: effectiveCount }, (_, i) => i as PaneIndex)
  const effectiveFocus: PaneIndex = focusedPane >= effectiveCount ? 0 : focusedPane

  return (
    <div className="flex flex-1 min-h-0 min-w-0">
      {indices.map((idx) => {
        const sid = paneAssignments[idx]
        const isFocused = idx === effectiveFocus
        return (
          <div
            key={idx}
            className={`flex flex-1 min-w-0 min-h-0 ${
              idx > 0 ? 'border-l border-border' : ''
            }`}
            onClick={() => onFocusPane(idx)}
          >
            {sid ? (
              <XtermPane
                key={sid}
                sessionId={sid}
                prefs={prefs}
                isFocused={isFocused}
                onFocus={() => onFocusPane(idx)}
                onConnectedChange={onConnectedChange}
                onReconnectAttempt={onReconnectAttempt}
                onReconnectExhausted={onReconnectExhausted}
                onError={onError}
                registerSend={registerSend}
                reconnectNonce={reconnectNonce}
              />
            ) : (
              <EmptyPane focused={isFocused} />
            )}
          </div>
        )
      })}
    </div>
  )
}

function EmptyPane({ focused }: { focused: boolean }) {
  return (
    <div className={`flex flex-1 min-w-0 min-h-0 items-center justify-center text-center text-text-muted text-xs px-4 ${
      focused ? 'ring-1 ring-inset ring-accent/40' : ''
    }`}>
      <div>
        <div className="text-text-secondary mb-1">Empty pane</div>
        <div>Click a tab to load it here.</div>
      </div>
    </div>
  )
}
