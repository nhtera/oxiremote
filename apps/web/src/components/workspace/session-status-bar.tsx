import type { Session, PaneCount } from '../../state/terminal-store'
import TransportPill from '../transport-pill'

interface Props {
  active: Session | null
  focusedSessionId: string | null
  isFocusedConnected: boolean
  paneCount: PaneCount
  err: string | null
  onReconnect: () => void
  onCloseFocused: () => void
  onPaneCountChange: (n: PaneCount) => void
}

// Per-session status bar pinned below the tab strip in the workspace.
// Renders the focused session's connection pill (Connected / Disconnected /
// Exited) plus the global LAN/Tunnel transport pill, and surfaces inline
// Reconnect / Close-tab actions when the focused session is unhealthy.
export default function SessionStatusBar({
  active,
  focusedSessionId,
  isFocusedConnected,
  paneCount,
  err,
  onReconnect,
  onCloseFocused,
  onPaneCountChange,
}: Props) {
  const isExited = active?.state === 'exited'
  const pillClass = isExited
    ? 'text-danger border-danger/30 bg-danger/10'
    : isFocusedConnected
      ? 'text-success border-success/30 bg-success/10'
      : 'text-warning border-warning/30 bg-warning/10'
  const pillLabel = !focusedSessionId
    ? 'No session'
    : isExited ? 'Exited' : isFocusedConnected ? 'Connected' : 'Disconnected'

  return (
    <div className="flex items-center gap-2 px-3 py-1 shrink-0 border-b border-border bg-surface/95 backdrop-blur">
      <span className={`text-[11px] px-2 py-0.5 rounded-full border ${pillClass}`}>
        {pillLabel}
      </span>
      <TransportPill compact />

      {!isFocusedConnected && focusedSessionId && !isExited && (
        <button
          onClick={onReconnect}
          className="btn-secondary text-xs py-0.5 px-2 text-warning"
        >
          Reconnect
        </button>
      )}
      {isExited && focusedSessionId && (
        <button
          onClick={onCloseFocused}
          className="btn-secondary text-xs py-0.5 px-2"
          title="The PTY has exited — close this tab"
        >
          Close tab
        </button>
      )}
      <SplitToggle value={paneCount} onChange={onPaneCountChange} />
      {active && (
        <span className="text-xs text-text-muted ml-auto">
          {(active.name ?? active.id.slice(0, 8))} · {active.cols}×{active.rows}
        </span>
      )}
      {err && <span className="text-danger text-xs ml-auto truncate max-w-50">{err}</span>}
    </div>
  )
}

function SplitToggle({ value, onChange }: { value: PaneCount; onChange: (n: PaneCount) => void }) {
  return (
    <div
      className="hidden md:inline-flex rounded-md border border-border overflow-hidden text-[11px]"
      role="tablist"
      aria-label="Pane split"
    >
      {([1, 2, 3] as PaneCount[]).map((n) => (
        <button
          key={n}
          onClick={() => onChange(n)}
          className={`px-2 py-0.5 transition-colors ${
            value === n
              ? 'bg-accent/15 text-accent'
              : 'text-text-muted hover:bg-surface-hover hover:text-text-primary'
          }`}
          role="tab"
          aria-selected={value === n}
          title={`${n}-pane split`}
        >
          {n}-up
        </button>
      ))}
    </div>
  )
}
