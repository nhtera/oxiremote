import { NavLink } from 'react-router-dom'
import {
  TerminalIcon,
  RemoteDesktopIcon,
  FilesIcon,
  GitIcon,
  PreviewIcon,
} from '../icons'

interface Props {
  hostId: string
}

interface Tab {
  to: string
  label: string
  Icon: (props: { size?: number }) => React.ReactNode
}

// Mobile bottom tab bar. Lives in the app-layout's last grid row so it stays
// mounted across surface navigation. Hidden on lg+ (desktop has top icon nav).
// Visible up to 1024px so tablet portrait users get the bar. Hit targets
// ≥44×44 px; safe-area-inset-bottom on iOS home-indicator devices.
export default function MobileBottomTabs({ hostId }: Props) {
  const tabs: Tab[] = [
    { to: `/h/${hostId}/workspace`, label: 'Terminal', Icon: TerminalIcon },
    { to: `/h/${hostId}/desktop`, label: 'Desktop', Icon: RemoteDesktopIcon },
    { to: `/h/${hostId}/files`, label: 'Files', Icon: FilesIcon },
    { to: `/h/${hostId}/git`, label: 'Git', Icon: GitIcon },
    { to: `/h/${hostId}/preview`, label: 'Sites', Icon: PreviewIcon },
  ]

  return (
    <nav
      className="lg:hidden border-t border-border bg-surface-alt/95 backdrop-blur-md flex items-stretch shrink-0"
      style={{ paddingBottom: 'env(safe-area-inset-bottom, 0px)' }}
      aria-label="Primary"
    >
      {tabs.map((tab) => (
        <NavLink
          key={tab.to}
          to={tab.to}
          className={({ isActive }) =>
            `relative flex-1 min-w-0 flex flex-col items-center justify-center gap-0.5 py-1.5 min-h-12 transition-colors ${
              isActive
                ? 'text-accent'
                : 'text-text-muted active:bg-surface-hover'
            }`
          }
        >
          {({ isActive }) => (
            <>
              {isActive && (
                <span
                  aria-hidden
                  className="absolute top-0 left-1/2 -translate-x-1/2 w-9 h-0.5 rounded-b-full bg-accent shadow-[0_0_12px_rgba(255,122,64,0.55)]"
                />
              )}
              <span
                className={`flex items-center justify-center w-9 h-9 rounded-lg transition-colors ${
                  isActive ? 'bg-accent/15' : ''
                }`}
              >
                <tab.Icon size={20} />
              </span>
              <span
                className={`text-[10px] leading-none ${
                  isActive ? 'font-semibold tracking-wide' : ''
                }`}
              >
                {tab.label}
              </span>
            </>
          )}
        </NavLink>
      ))}
    </nav>
  )
}
