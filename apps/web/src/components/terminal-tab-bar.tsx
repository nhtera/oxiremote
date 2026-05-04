import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import type { Session } from '../state/terminal-store'
import { RemoteDesktopIcon } from './icons'
import AgentDetectedBadge from './agent-detected-badge'
import NotifyOnFinishToggle from './notify-on-finish-toggle'

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
  /** When supplied, render a Monitor icon between New-tab and gear that
   *  navigates to the remote-desktop surface for this host. */
  hostId?: string
  /** Hide the Monitor button (no RD permission / disabled). */
  desktopAvailable?: boolean
  /** Called when the notify-on-finish toggle changes so the parent can persist. */
  onNotifyToggle?: (sessionId: string, enabled: boolean) => void
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
  return s.state === 'active' ? 'connected' : 'idle'
}

type ContextMenuState = {
  sessionId: string
  x: number
  y: number
} | null

export default function TerminalTabBar({
  sessions, activeId, isActiveConnected, connectedById, reconnectingById,
  onSelect, onClose, onNew, onRename, onOpenSettings,
  hostId, desktopAvailable = true,
  onNotifyToggle,
}: Props) {
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editValue, setEditValue] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const [contextMenu, setContextMenu] = useState<ContextMenuState>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  function startRename(s: Session) {
    setEditingId(s.id)
    setEditValue(s.name ?? s.id.slice(0, 8))
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

  function openContextMenu(e: React.MouseEvent, sessionId: string) {
    e.preventDefault()
    e.stopPropagation()
    setContextMenu({ sessionId, x: e.clientX, y: e.clientY })
  }

  // Close context menu on outside click or Escape.
  useEffect(() => {
    if (!contextMenu) return
    function handleClickOutside(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setContextMenu(null)
      }
    }
    function handleEsc(e: KeyboardEvent) {
      if (e.key === 'Escape') setContextMenu(null)
    }
    document.addEventListener('mousedown', handleClickOutside)
    document.addEventListener('keydown', handleEsc)
    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
      document.removeEventListener('keydown', handleEsc)
    }
  }, [contextMenu])

  const contextSession = contextMenu
    ? sessions.find((s) => s.id === contextMenu.sessionId)
    : null

  return (
    <>
      <div className="flex items-center gap-0.5 overflow-x-auto border-b border-border bg-surface-alt shrink-0 min-h-[36px]">
        {sessions.map((s) => {
          const isActive = s.id === activeId
          let dot = resolveDot(s, connectedById, reconnectingById)
          if (isActive && !connectedById && !reconnectingById && isActiveConnected && s.state !== 'exited') {
            dot = 'connected'
          }
          return (
            <div
              key={s.id}
              onClick={() => onSelect(s.id)}
              onDoubleClick={() => startRename(s)}
              onContextMenu={(e) => openContextMenu(e, s.id)}
              className={`flex items-center gap-1.5 px-2 py-1 shrink-0 cursor-pointer border-r border-border text-xs select-none min-w-[80px] max-w-[180px] group transition-colors ${
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

              {/* Agent badge — shown when an agent CLI is detected */}
              {s.detected_agent && (
                <AgentDetectedBadge agentName={s.detected_agent} />
              )}

              {/* Close button */}
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

        {/* Quick-jump to remote desktop */}
        {hostId && desktopAvailable && (
          <Link
            to={`/h/${hostId}/desktop`}
            className="px-2 py-1 shrink-0 text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors flex items-center"
            title="Open remote desktop"
            aria-label="Open remote desktop"
          >
            <RemoteDesktopIcon size={14} />
          </Link>
        )}

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

      {/* Tab context menu — portal rendered at cursor position */}
      {contextMenu && contextSession && (
        <div
          ref={menuRef}
          role="menu"
          className="fixed z-50 min-w-[180px] bg-surface border border-border rounded-lg shadow-lg py-1 text-xs"
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <button
            role="menuitem"
            onClick={() => { startRename(contextSession); setContextMenu(null) }}
            className="flex items-center gap-2 w-full px-3 py-2 text-left text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            Rename tab
          </button>
          <button
            role="menuitem"
            onClick={() => { onClose(contextSession.id); setContextMenu(null) }}
            className="flex items-center gap-2 w-full px-3 py-2 text-left text-text-secondary hover:bg-surface-hover hover:text-danger transition-colors"
          >
            Close session
          </button>

          {/* Agent notify-on-finish toggle — only shown when agent detected */}
          {contextSession.detected_agent && (
            <>
              <div className="my-1 border-t border-border" />
              <NotifyOnFinishToggle
                sessionId={contextSession.id}
                agentName={contextSession.detected_agent}
                enabled={!!contextSession.notify_on_agent_end}
                onToggled={(enabled) => {
                  onNotifyToggle?.(contextSession.id, enabled)
                  setContextMenu(null)
                }}
              />
            </>
          )}
        </div>
      )}
    </>
  )
}
