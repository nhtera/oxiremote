import { useRef, useState } from 'react'
import type { Session } from '../state/terminal-store'

type Props = {
  sessions: Session[]
  activeId: string | null
  isActiveConnected?: boolean
  onSelect: (id: string) => void
  onClose: (id: string) => void
  onNew: () => void
  onRename: (id: string, name: string) => void
  onOpenSettings: () => void
}

function statusDot(state: Session['state']): string {
  // Conventional traffic-light mapping: active=green (running), exited=red
  // (process gone), idle=muted (alive but not currently focused).
  if (state === 'active') return 'bg-success'
  if (state === 'exited') return 'bg-danger'
  return 'bg-text-muted'
}

function statusLabel(state: Session['state']): string {
  if (state === 'active') return 'Running'
  if (state === 'exited') return 'Exited'
  return 'Idle'
}

export default function TerminalTabBar({
  sessions, activeId, isActiveConnected, onSelect, onClose, onNew, onRename, onOpenSettings,
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
        // The server flips state→"active" only on recent PTY output, so an
        // attached-but-quiet shell shows "idle". For the focused tab we treat
        // a live WS as the source of truth so the dot matches the Connected
        // pill below.
        const effectiveState: Session['state'] =
          isActive && isActiveConnected && s.state !== 'exited' ? 'active' : s.state
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
            className={`w-1.5 h-1.5 rounded-full shrink-0 ${statusDot(effectiveState)}`}
            title={statusLabel(effectiveState)}
            aria-label={statusLabel(effectiveState)}
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
