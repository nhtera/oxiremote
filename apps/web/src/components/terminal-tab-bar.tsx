import { useRef, useState } from 'react'
import type { Session } from '../state/terminal-store'

type Props = {
  sessions: Session[]
  activeId: string | null
  isActiveConnected?: boolean
  /** Per-session WS connection state — overrides server `state` for tabs whose
   *  session is mounted in a pane. Lets the dot turn green the moment the WS
   *  opens instead of waiting for the server's "active on output" heuristic. */
  connectedById?: Record<string, boolean>
  /** Per-session reconnect-in-progress flag. Drives the orange dot. */
  reconnectingById?: Record<string, boolean>
  onSelect: (id: string) => void
  onClose: (id: string) => void
  onNew: () => void
  onRename: (id: string, name: string) => void
  onOpenSettings: () => void
}

type DotKind = 'connected' | 'reconnecting' | 'exited' | 'idle'

function dotClass(kind: DotKind): string {
  if (kind === 'connected') return 'bg-success'
  if (kind === 'reconnecting') return 'bg-warning'
  if (kind === 'exited') return 'bg-danger'
  return 'bg-text-muted'
}

function dotLabel(kind: DotKind): string {
  if (kind === 'connected') return 'Connected'
  if (kind === 'reconnecting') return 'Reconnecting'
  if (kind === 'exited') return 'Exited'
  return 'Idle'
}

function resolveDot(
  s: Session,
  connectedById: Record<string, boolean> | undefined,
  reconnectingById: Record<string, boolean> | undefined,
): DotKind {
  if (s.state === 'exited') return 'exited'
  if (reconnectingById?.[s.id]) return 'reconnecting'
  if (connectedById?.[s.id]) return 'connected'
  // No WS for this tab (not mounted in any pane) — fall back to server state.
  return s.state === 'active' ? 'connected' : 'idle'
}

export default function TerminalTabBar({
  sessions, activeId, isActiveConnected, connectedById, reconnectingById,
  onSelect, onClose, onNew, onRename, onOpenSettings,
}: Props) {
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editValue, setEditValue] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  function startRename(s: Session) {
    setEditingId(s.id)
    setEditValue(s.name ?? s.id.slice(0, 8))
    // Focus after render
    setTimeout(() => inputRef.current?.select(), 0)
  }

  function commitRename(id: string) {
    const trimmed = editValue.trim()
    if (trimmed.length > 0 && trimmed.length <= 64) onRename(id, trimmed)
    setEditingId(null)
  }

  function handleKeyDown(e: React.KeyboardEvent, id: string) {
    if (e.key === 'Enter') { e.preventDefault(); commitRename(id) }
    if (e.key === 'Escape') setEditingId(null)
  }

  return (
    <div className="flex items-center gap-0.5 overflow-x-auto border-b border-border bg-surface-alt shrink-0 min-h-[36px]">
      {sessions.map((s) => {
        const isActive = s.id === activeId
        let dot = resolveDot(s, connectedById, reconnectingById)
        // Focused-tab back-compat: when callers pass only isActiveConnected
        // (slice 1 callers), still upgrade the dot to green/orange so the
        // pill above and the dot agree.
        if (isActive && !connectedById && !reconnectingById && isActiveConnected && s.state !== 'exited') {
          dot = 'connected'
        }
        return (
        <div
          key={s.id}
          onClick={() => onSelect(s.id)}
          onDoubleClick={() => startRename(s)}
          className={`flex items-center gap-1.5 px-2 py-1 shrink-0 cursor-pointer border-r border-border text-xs select-none min-w-[80px] max-w-[160px] group transition-colors ${
            isActive
              ? 'bg-surface text-text-primary'
              : 'text-text-secondary hover:bg-surface-hover'
          }`}
        >
          {/* Status dot */}
          <span
            className={`w-1.5 h-1.5 rounded-full shrink-0 ${dotClass(dot)}`}
            title={dotLabel(dot)}
            aria-label={dotLabel(dot)}
          />

          {/* Tab name or inline rename input */}
          {editingId === s.id ? (
            <input
              ref={inputRef}
              value={editValue}
              onChange={(e) => setEditValue(e.target.value)}
              onBlur={() => commitRename(s.id)}
              onKeyDown={(e) => handleKeyDown(e, s.id)}
              onClick={(e) => e.stopPropagation()}
              className="flex-1 min-w-0 bg-surface-hover text-text-primary text-xs px-1 rounded outline-none border border-accent/40"
              maxLength={64}
            />
          ) : (
            <span className="flex-1 min-w-0 truncate">
              {s.name ?? s.id.slice(0, 8)}
            </span>
          )}

          {/* Close button — always visible (touch devices have no hover, and
              hiding it on desktop made the tap target unreliable). */}
          <button
            onClick={(e) => { e.stopPropagation(); onClose(s.id) }}
            className="shrink-0 w-4 h-4 flex items-center justify-center rounded text-text-muted hover:text-danger hover:bg-surface-hover transition-colors leading-none text-sm"
            title="Close session"
            aria-label="Close session"
          >
            ×
          </button>
        </div>
        )
      })}

      {/* New tab */}
      <button
        onClick={onNew}
        className="px-2.5 py-1 shrink-0 text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors text-sm leading-none"
        title="New session"
      >
        +
      </button>

      {/* Spacer pushes gear to the right */}
      <div className="flex-1" />

      {/* Settings gear */}
      <button
        onClick={onOpenSettings}
        className="px-2.5 py-1 shrink-0 text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors text-sm leading-none"
        title="Terminal settings"
      >
        ⚙
      </button>
    </div>
  )
}
