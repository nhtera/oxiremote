import { NavLink, Outlet } from 'react-router-dom'
import AgentHeader from './agent-header'
import UpdateBanner from './update-banner'
import { HomeIcon, DevicesIcon, LogsIcon, SettingsIcon } from './icons'

type IconCmp = (props: { size?: number }) => React.ReactNode

// Sidebar layout for localhost-only `/agent/*` pages (the host dashboard).
// Distinct from AppLayout — this SPA tree is not reachable via the tunnel
// (the agent enforces 403 at the network layer), so we don't render the
// mobile nav bar or push/install banners that only make sense on phones.
//
// Header is now AgentHeader (sticky, version chip, tunnel pill, quick-actions,
// Stop-agent button). UpdateBanner sits between header and content area.
export default function AgentLayout() {
  const navItems: { to: string; label: string; Icon: IconCmp; exact: boolean }[] = [
    { to: '/agent', label: 'Home', Icon: HomeIcon, exact: true },
    { to: '/agent/devices', label: 'Devices', Icon: DevicesIcon, exact: false },
    { to: '/agent/logs', label: 'Logs', Icon: LogsIcon, exact: false },
    { to: '/agent/settings', label: 'Settings', Icon: SettingsIcon, exact: false },
  ]

  return (
    <div className="flex flex-col min-h-dvh">
      <AgentHeader />
      <UpdateBanner />

      <div className="flex flex-1 min-h-0">
        <nav className="flex flex-col w-56 border-r border-border bg-surface-alt shrink-0">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.exact}
              className={({ isActive }) =>
                `px-4 py-2.5 text-sm flex items-center gap-2 transition-colors ${
                  isActive
                    ? 'bg-surface-hover text-text-primary'
                    : 'text-text-secondary hover:bg-surface-hover hover:text-text-primary'
                }`
              }
            >
              <span className="w-5 h-5 shrink-0 flex items-center justify-center">
                <item.Icon size={16} />
              </span>
              {item.label}
            </NavLink>
          ))}
        </nav>

        <main className="flex-1 min-h-0 overflow-auto">
          <Outlet />
        </main>
      </div>
    </div>
  )
}
