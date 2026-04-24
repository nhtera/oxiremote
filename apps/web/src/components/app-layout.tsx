import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import PushPermissionBanner from './push-permission-banner'
import InstallPwaBanner from './install-pwa-banner'
import { clearApiKey } from '../lib/api-client'

export default function AppLayout() {
  const navigate = useNavigate()
  const { currentHostId, label } = useHostStore()

  // Display label or first 8 chars of host_id as fallback
  const hostChip = label ?? (currentHostId ? currentHostId.slice(0, 8) : null)

  // Nav items: use host-scoped paths when host is known, else legacy (which redirect)
  const base = currentHostId ? `/h/${currentHostId}` : ''
  const navItems = [
    { to: '/', label: 'Home', icon: '⌂', exact: true },
    { to: `${base}/terminal`, label: 'Terminal', icon: '▸', exact: false },
    { to: `${base}/git`, label: 'Git', icon: '⎇', exact: false },
    { to: `${base}/files`, label: 'Files', icon: '◫', exact: false },
    { to: `${base}/preview`, label: 'Preview', icon: '◉', exact: false },
  ]

  const handleLogout = async () => {
    await fetch('/api/auth/logout', { method: 'POST' })
    clearApiKey()
    navigate('/login')
  }

  return (
    <div className="flex flex-col md:flex-row min-h-dvh">
      {/* Desktop sidebar */}
      <nav className="hidden md:flex flex-col w-48 border-r border-border bg-surface-alt shrink-0">
        <div className="px-4 py-3 text-sm font-semibold text-accent tracking-wide">
          OxiRemote
        </div>
        {/* Host chip */}
        {hostChip && (
          <div className="mx-3 mb-1 px-2 py-1 rounded-md bg-surface border border-border text-[11px] text-text-muted truncate" title={currentHostId ?? ''}>
            {hostChip}
          </div>
        )}
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
        <div className="mt-auto border-t border-border">
          <button
            onClick={handleLogout}
            className="w-full px-4 py-2.5 text-sm text-left text-text-muted hover:text-danger hover:bg-surface-hover transition-colors"
          >
            Logout
          </button>
        </div>
      </nav>

      {/* Main content */}
      <main className="flex-1 min-h-0 overflow-auto pb-20 md:pb-0">
        <InstallPwaBanner />
        <PushPermissionBanner />
        <Outlet />
      </main>

      {/* Mobile bottom tabs */}
      <nav className="md:hidden fixed bottom-0 inset-x-0 bg-surface-alt border-t border-border flex z-50" style={{ paddingBottom: 'env(safe-area-inset-bottom, 0px)' }}>
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.exact}
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
