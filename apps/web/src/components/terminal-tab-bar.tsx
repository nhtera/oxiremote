import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import type { Session } from '../state/terminal-store'
import { PaperclipIcon, RemoteDesktopIcon } from './icons'
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
  /** Desktop attach affordance — opens the picker (modal variant) so PC users
   *  have parity with the mobile composer's paperclip. Hidden when no
   *  workspace is active or the handler is omitted (mobile uses the composer). */
  onOpenAttach?: () => void
  /** Set false when there's no active workspace; renders the paperclip
   *  disabled with a tooltip. Defaults to true. */
  attachAvailable?: boolean
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
  onOpenAttach, attachAvailable = true,
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
      <div className="flex items-center gap-0.5 overflow-x-auto border-b border-border bg-surface-alt shrink-0 min-h-[40px] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
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
              className={`relative flex items-center gap-1.5 px-2.5 py-1.5 shrink-0 cursor-pointer text-[13px] select-none min-w-[84px] max-w-[180px] group transition-colors ${
                isActive
                  ? 'text-accent font-medium'
                  : 'text-text-secondary hover:bg-surface-hover hover:text-text-primary'
              }`}
            >
              {isActive && (
                <span aria-hidden className="absolute bottom-0 left-2 right-2 h-0.5 rounded-t-full bg-accent" />
              )}
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

        {/* New tab — accent-tinted so it's the obvious affordance after tabs run out */}
        <button
          onClick={onNew}
          className="ml-1 mr-0.5 shrink-0 inline-flex items-center justify-center w-8 h-8 rounded-md text-accent bg-accent/10 hover:bg-accent/20 transition-colors leading-none"
          title="New session"
          aria-label="New session"
        >
          <svg viewBox="0 0 16 16" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M8 3v10" /><path d="M3 8h10" />
          </svg>
        </button>

        {/* Quick-jump to remote desktop */}
        {hostId && desktopAvailable && (
          <Link
            to={`/h/${hostId}/desktop`}
            className="shrink-0 inline-flex items-center justify-center w-8 h-8 rounded-md text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
            title="Open remote desktop"
            aria-label="Open remote desktop"
          >
            <RemoteDesktopIcon size={16} />
          </Link>
        )}

        {/* Spacer pushes gear to the right */}
        <div className="flex-1" />

        {/* Desktop-only attach paperclip. Mobile uses the composer's paperclip;
            hide here below md so the affordance isn't duplicated on the bottom
            edge. Disabled state still shows the button so users see the
            affordance and learn to pick a workspace. */}
        {onOpenAttach && (
          <button
            onClick={onOpenAttach}
            disabled={!attachAvailable}
            className={`hidden md:inline-flex shrink-0 items-center justify-center w-8 h-8 rounded-md transition-colors ${
              attachAvailable
                ? 'text-text-muted hover:text-accent hover:bg-surface-hover'
                : 'text-text-muted/40 cursor-not-allowed'
            }`}
            title={attachAvailable ? 'Attach file (paste / drag also works)' : 'Pick a workspace first'}
            aria-label="Attach file"
          >
            <PaperclipIcon size={16} />
          </button>
        )}

        {/* Settings gear */}
        <button
          onClick={onOpenSettings}
          className="mr-1 shrink-0 inline-flex items-center justify-center w-8 h-8 rounded-md text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
          title="Terminal settings"
          aria-label="Terminal settings"
        >
          <svg viewBox="0 0 16 16" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="2" />
            <path d="M8 1v2 M8 13v2 M1 8h2 M13 8h2 M3 3l1.5 1.5 M11.5 11.5L13 13 M3 13l1.5-1.5 M11.5 4.5L13 3" />
          </svg>
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
