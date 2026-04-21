import { NavLink, Outlet } from 'react-router-dom'

const navItems = [
  { to: '/', label: 'Home', icon: '⌂' },
  { to: '/terminal', label: 'Terminal', icon: '▸' },
  { to: '/git', label: 'Git', icon: '⎇' },
  { to: '/files', label: 'Files', icon: '◫' },
  { to: '/preview', label: 'Preview', icon: '◉' },
]

export default function AppLayout() {
  return (
    <div className="flex flex-col md:flex-row min-h-dvh">
      {/* Desktop sidebar */}
      <nav className="hidden md:flex flex-col w-48 border-r border-border bg-surface-alt shrink-0">
        <div className="px-4 py-3 text-sm font-semibold text-accent tracking-wide">
          OxiRemote
        </div>
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === '/'}
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

      {/* Main content */}
      <main className="flex-1 min-h-0 overflow-auto pb-16 md:pb-0">
        <Outlet />
      </main>

      {/* Mobile bottom tabs */}
      <nav className="md:hidden fixed bottom-0 inset-x-0 bg-surface-alt border-t border-border flex z-50">
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === '/'}
            className={({ isActive }) =>
              `flex-1 flex flex-col items-center py-2 text-xs transition-colors ${
                isActive ? 'text-accent' : 'text-text-muted'
              }`
            }
          >
            <span className="text-lg leading-none mb-0.5">{item.icon}</span>
            {item.label}
          </NavLink>
        ))}
      </nav>
    </div>
  )
}
