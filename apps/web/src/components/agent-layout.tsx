import { useEffect, useState } from 'react'
import { NavLink, Outlet } from 'react-router-dom'
import { TunnelStatusPill } from './tunnel-status-card'
import AgentDisconnectButton from './agent-disconnect-button'
import { HomeIcon, DevicesIcon, LogsIcon, SettingsIcon } from './icons'

type IconCmp = (props: { size?: number }) => React.ReactNode

// Sidebar layout for localhost-only `/agent/*` pages (the host dashboard).
// Distinct from AppLayout — this SPA tree is not reachable via the tunnel
// (the agent enforces 403 at the network layer), so we don't render the
// mobile nav bar or push/install banners that only make sense on phones.
//
// Top header carries a global tunnel-status pill + Stop-agent button so the
// operator can read connectivity at a glance and shut the agent from any
// /agent/* route, not just the home page.
export default function AgentLayout() {
  // Cheap one-shot fetch — version is fixed for the process lifetime, no SSE.
  const [version, setVersion] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (!cancelled && data?.version) setVersion(data.version as string)
      })
      .catch(() => { /* version pill is non-critical */ })
    return () => { cancelled = true }
  }, [])

  const navItems: { to: string; label: string; Icon: IconCmp; exact: boolean }[] = [
    { to: '/agent', label: 'Home', Icon: HomeIcon, exact: true },
    { to: '/agent/devices', label: 'Devices', Icon: DevicesIcon, exact: false },
    { to: '/agent/logs', label: 'Logs', Icon: LogsIcon, exact: false },
    { to: '/agent/settings', label: 'Settings', Icon: SettingsIcon, exact: false },
  ]

  return (
    <div className="flex flex-col min-h-dvh">
      <header className="flex items-center gap-3 px-4 py-2.5 border-b border-border bg-surface-alt">
        <span
          aria-hidden
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-white shrink-0 shadow-[0_4px_14px_-6px_rgba(255,122,64,0.55)]"
        >
          <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="4" width="18" height="13" rx="2" />
            <path d="M8 20h8" />
            <path d="M12 17v3" />
          </svg>
        </span>
        <div className="min-w-0">
          <div className="text-sm font-semibold text-text-primary tracking-tight leading-tight flex items-center gap-2">
            <span>OxiRemote <span className="text-text-muted font-normal">host</span></span>
            {version && (
              <span className="text-[10px] font-medium text-accent bg-accent/10 border border-accent/30 rounded px-1.5 py-0.5 leading-none">
                v{version}
              </span>
            )}
          </div>
          <div className="text-[11px] text-text-muted leading-tight">localhost dashboard</div>
        </div>
        <div className="flex-1" />
        <TunnelStatusPill />
        <AgentDisconnectButton />
      </header>

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
