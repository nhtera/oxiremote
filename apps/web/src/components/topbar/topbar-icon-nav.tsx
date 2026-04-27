import { NavLink } from 'react-router-dom'
import {
  TerminalIcon,
  RemoteDesktopIcon,
  GitIcon,
  FilesIcon,
  PreviewIcon,
  SettingsIcon,
} from '../icons'

type IconCmp = (props: { size?: number }) => React.ReactNode

interface NavSpec {
  to: string
  label: string
  Icon: IconCmp
}

interface Props {
  hostId: string
  compact?: boolean
}

// Top icon strip — Terminal / Desktop / Git / Files / Preview / Settings.
// Active route shows a 2px coral underline so the operator can spot the
// current surface without reading labels.
export default function TopbarIconNav({ hostId, compact = false }: Props) {
  const items: NavSpec[] = [
    { to: `/h/${hostId}/workspace`, label: 'Terminal', Icon: TerminalIcon },
    { to: `/h/${hostId}/desktop`, label: 'Desktop', Icon: RemoteDesktopIcon },
    { to: `/h/${hostId}/git`, label: 'Git', Icon: GitIcon },
    { to: `/h/${hostId}/files`, label: 'Files', Icon: FilesIcon },
    { to: `/h/${hostId}/preview`, label: 'Preview', Icon: PreviewIcon },
    { to: `/h/${hostId}/dashboard`, label: 'Settings', Icon: SettingsIcon },
  ]

  return (
    <nav className="flex items-center gap-0.5">
      {items.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          title={item.label}
          aria-label={item.label}
          className={({ isActive }) =>
            `relative flex items-center justify-center ${compact ? 'h-9 w-9' : 'h-10 w-10'} rounded-md transition-colors ${
              isActive
                ? 'text-accent'
                : 'text-text-muted hover:text-text-primary hover:bg-surface-hover'
            }`
          }
        >
          {({ isActive }) => (
            <>
              <item.Icon size={compact ? 16 : 18} />
              {isActive && (
                <span
                  aria-hidden
                  className="absolute -bottom-0.5 left-1/2 -translate-x-1/2 w-5 h-0.5 rounded-full bg-accent"
                />
              )}
            </>
          )}
        </NavLink>
      ))}
    </nav>
  )
}
