import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import Sheet from '../ui/sheet'
import GitSheet from './git-sheet'
import PreviewSheet from './preview-sheet'
import FilesSheet from './files-sheet'
import {
  TerminalIcon,
  RemoteDesktopIcon,
  GitIcon,
  FilesIcon,
  PreviewIcon,
  DevicesIcon,
  LogsIcon,
  SettingsIcon,
} from '../icons'

type Props = {
  open: boolean
  hostId: string
  onClose: () => void
}

type IconCmp = (props: { size?: number }) => React.ReactNode

type Item = {
  label: string
  description?: string
  Icon: IconCmp
  /** Either a route to navigate to, or a sheet key to open inline. */
  action: { kind: 'route'; path: string } | { kind: 'sheet'; key: 'git' | 'preview' | 'files' }
}

// Right-side settings drawer triggered from the topbar gear icon. Items
// either route to a full page (Workspace, Desktop, Files, Dashboard) or open
// a compact-mode sheet (Git, Sites) so the underlying terminal session stays
// alive behind the overlay.
export default function GearDrawer({ open, hostId, onClose }: Props) {
  const navigate = useNavigate()
  const [activeSheet, setActiveSheet] = useState<'git' | 'preview' | 'files' | null>(null)

  function handle(item: Item) {
    if (item.action.kind === 'route') {
      navigate(item.action.path)
      onClose()
      return
    }
    setActiveSheet(item.action.key)
  }

  const surfaces: Item[] = [
    { label: 'Workspace', description: 'Terminal sessions', Icon: TerminalIcon, action: { kind: 'route', path: `/h/${hostId}/workspace` } },
    { label: 'Remote Desktop', Icon: RemoteDesktopIcon, action: { kind: 'route', path: `/h/${hostId}/desktop` } },
    { label: 'Files', description: 'Browse, edit, upload', Icon: FilesIcon, action: { kind: 'sheet', key: 'files' } },
    { label: 'Git', description: 'Status, diff, commit', Icon: GitIcon, action: { kind: 'sheet', key: 'git' } },
    { label: 'Sites (Preview)', description: 'Local servers, share links', Icon: PreviewIcon, action: { kind: 'sheet', key: 'preview' } },
  ]
  const ops: Item[] = [
    { label: 'Dashboard', description: 'Devices, tunnel, settings', Icon: SettingsIcon, action: { kind: 'route', path: `/h/${hostId}/dashboard` } },
    { label: 'Devices', description: 'Trusted devices, revoke', Icon: DevicesIcon, action: { kind: 'route', path: `/h/${hostId}/devices` } },
    { label: 'Logs', description: 'Session activity', Icon: LogsIcon, action: { kind: 'route', path: `/h/${hostId}/logs` } },
  ]

  return (
    <>
      <Sheet
        open={open}
        onClose={onClose}
        side="right"
        ariaLabelledBy="gear-drawer-title"
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-border sticky top-0 bg-surface z-10">
          <div id="gear-drawer-title" className="text-text-primary font-semibold text-sm">
            Settings
          </div>
          <button
            onClick={onClose}
            aria-label="Close settings"
            className="text-text-muted hover:text-text-primary text-lg leading-none w-7 h-7 flex items-center justify-center rounded hover:bg-surface-hover"
          >
            ×
          </button>
        </div>

        <Group label="Surfaces">
          {surfaces.map((it) => (
            <Row key={it.label} item={it} onClick={() => handle(it)} />
          ))}
        </Group>

        <Group label="Operations">
          {ops.map((it) => (
            <Row key={it.label} item={it} onClick={() => handle(it)} />
          ))}
        </Group>
      </Sheet>

      <GitSheet open={activeSheet === 'git'} onClose={() => setActiveSheet(null)} />
      <PreviewSheet open={activeSheet === 'preview'} onClose={() => setActiveSheet(null)} />
      <FilesSheet open={activeSheet === 'files'} onClose={() => setActiveSheet(null)} />
    </>
  )
}

function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="py-2">
      <div className="px-4 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
        {label}
      </div>
      <div>{children}</div>
    </div>
  )
}

function Row({ item, onClick }: { item: Item; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="w-full flex items-center gap-3 px-4 py-2.5 text-left hover:bg-surface-hover transition-colors"
    >
      <span className="text-text-muted shrink-0"><item.Icon size={18} /></span>
      <span className="flex-1 min-w-0">
        <div className="text-text-primary text-sm leading-tight truncate">{item.label}</div>
        {item.description && (
          <div className="text-text-muted text-[11px] truncate">{item.description}</div>
        )}
      </span>
      <span className="text-text-muted shrink-0" aria-hidden>›</span>
    </button>
  )
}
