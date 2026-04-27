import { useNavigate } from 'react-router-dom'
import Sheet from '../ui/sheet'
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
  path: string
  trailing?: string
}

// Right-side settings drawer triggered from the topbar gear icon. Items
// route to existing pages for now; Phase 07 slice B swaps Files/Git/Preview
// over to compact-mode sheets so the terminal session stays alive behind.
export default function GearDrawer({ open, hostId, onClose }: Props) {
  const navigate = useNavigate()

  function go(path: string) {
    navigate(path)
    onClose()
  }

  const surfaces: Item[] = [
    { label: 'Workspace', description: 'Terminal sessions', Icon: TerminalIcon, path: `/h/${hostId}/workspace` },
    { label: 'Remote Desktop', Icon: RemoteDesktopIcon, path: `/h/${hostId}/desktop` },
    { label: 'Files', Icon: FilesIcon, path: `/h/${hostId}/files` },
    { label: 'Git', Icon: GitIcon, path: `/h/${hostId}/git` },
    { label: 'Sites (Preview)', Icon: PreviewIcon, path: `/h/${hostId}/preview` },
  ]
  const ops: Item[] = [
    { label: 'Dashboard', description: 'Devices, tunnel, settings', Icon: SettingsIcon, path: `/h/${hostId}/dashboard` },
    { label: 'Devices', Icon: DevicesIcon, path: `/h/${hostId}/dashboard` },
    { label: 'Logs', Icon: LogsIcon, path: `/h/${hostId}/dashboard` },
  ]

  return (
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
          <Row key={it.label} item={it} onClick={() => go(it.path)} />
        ))}
      </Group>

      <Group label="Operations">
        {ops.map((it) => (
          <Row key={it.label} item={it} onClick={() => go(it.path)} />
        ))}
      </Group>
    </Sheet>
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
      {item.trailing && (
        <span className="text-text-muted text-xs shrink-0">{item.trailing}</span>
      )}
      <span className="text-text-muted shrink-0" aria-hidden>›</span>
    </button>
  )
}
