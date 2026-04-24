import { NavLink, Outlet } from 'react-router-dom'

// Sidebar layout for localhost-only `/agent/*` pages (the host dashboard).
// Distinct from AppLayout — this SPA tree is not reachable via the tunnel
// (the agent enforces 403 at the network layer), so we don't render the
// mobile nav bar or push/install banners that only make sense on phones.
export default function AgentLayout() {
  const navItems = [
    { to: '/agent', label: 'Home', icon: '⌂', exact: true },
    { to: '/agent/devices', label: 'Devices', icon: '◉', exact: false },
    { to: '/agent/settings', label: 'Settings', icon: '⚙', exact: false },
  ]

  return (
    <div className="flex min-h-dvh">
      <nav className="flex flex-col w-56 border-r border-border bg-surface-alt shrink-0">
        <div className="px-4 py-3 flex items-center gap-2">
          <span className="inline-block h-2.5 w-2.5 rounded-full bg-accent" />
          <div className="text-sm font-semibold text-accent tracking-wide">
            Host Dashboard
          </div>
        </div>
        <div className="mx-3 mb-2 px-2 py-1 rounded-md bg-surface border border-border text-[11px] text-text-muted">
          localhost only
        </div>
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
            <span className="text-base w-5 text-center">{item.icon}</span>
            {item.label}
          </NavLink>
        ))}
      </nav>

      <main className="flex-1 min-h-0 overflow-auto">
        <Outlet />
      </main>
    </div>
  )
}
