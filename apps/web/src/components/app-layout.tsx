import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import PushPermissionBanner from './push-permission-banner'
import InstallPwaBanner from './install-pwa-banner'
import { clearApiKey } from '../lib/api-client'
import {
  HomeIcon,
  TerminalIcon,
  GitIcon,
  FilesIcon,
  PreviewIcon,
} from './icons'

type IconCmp = (props: { size?: number }) => React.ReactNode

export default function AppLayout() {
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const { currentHostId, label } = useHostStore()

  // Push banner is a setup nudge, not an in-flow nag. Only show on home pages —
  // the workspace pages (terminal/files/git/preview/desktop) are mid-task.
  const isHomeRoute =
    pathname === '/' ||
    /^\/h\/[^/]+\/?$/.test(pathname)

  // Display label or first 8 chars of host_id as fallback
  const hostChip = label ?? (currentHostId ? currentHostId.slice(0, 8) : null)

  // Nav items: use host-scoped paths when host is known, else legacy (which redirect)
  const base = currentHostId ? `/h/${currentHostId}` : ''
  const navItems: { to: string; label: string; Icon: IconCmp; exact: boolean }[] = [
    { to: '/', label: 'Home', Icon: HomeIcon, exact: true },
    { to: `${base}/terminal`, label: 'Terminal', Icon: TerminalIcon, exact: false },
    { to: `${base}/git`, label: 'Git', Icon: GitIcon, exact: false },
    { to: `${base}/files`, label: 'Files', Icon: FilesIcon, exact: false },
    { to: `${base}/preview`, label: 'Preview', Icon: PreviewIcon, exact: false },
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
        <div className="px-4 py-3.5 flex items-center gap-2.5">
          <span
            aria-hidden
            className="inline-flex h-7 w-7 items-center justify-center rounded-md bg-accent text-white shadow-[0_4px_14px_-6px_rgba(255,122,64,0.55)]"
          >
            <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round">
              <rect x="3" y="4" width="18" height="13" rx="2" />
              <path d="M8 20h8" />
              <path d="M12 17v3" />
            </svg>
          </span>
          <span className="text-sm font-semibold text-text-primary tracking-tight">
            OxiRemote
          </span>
        </div>
        {/* Host chip with status dot — green when a host_id is resolved (the
            agent is reachable to this client), muted otherwise. */}
        {hostChip && (
          <div
            className="mx-3 mb-1 px-2 py-1 rounded-md bg-surface border border-border text-[11px] flex items-center gap-1.5"
            title={currentHostId ?? ''}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${currentHostId ? 'bg-success' : 'bg-text-muted'}`} />
            <span className="truncate text-text-muted">{hostChip}</span>
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
            <span className="w-5 h-5 shrink-0 flex items-center justify-center">
              <item.Icon size={16} />
            </span>
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
        {isHomeRoute && <PushPermissionBanner />}
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
            <span className="mb-0.5">
              <item.Icon size={18} />
            </span>
            {item.label}
          </NavLink>
        ))}
      </nav>
    </div>
  )
}
