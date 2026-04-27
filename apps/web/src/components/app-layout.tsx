import { Outlet, useLocation } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import PushPermissionBanner from './push-permission-banner'
import InstallPwaBanner from './install-pwa-banner'
import TopbarHostMenu from './topbar/topbar-host-menu'
import TopbarIconNav from './topbar/topbar-icon-nav'
import TopbarMobile from './topbar/topbar-mobile'

// Workspace-centric chrome (Phase 05). The sidebar is gone; navigation lives
// in a 48px top bar with a host-switcher dropdown on the left and an icon
// strip on the right. Mobile collapses to a 44px variant. Banners
// (PWA/push) sit between the top bar and the routed page so they don't
// interfere with the new chrome.
export default function AppLayout() {
  const { pathname } = useLocation()
  const { currentHostId } = useHostStore()

  // Push banner is a setup nudge, not an in-flow nag. Show it only when the
  // operator is on the dashboard surface (the calm one).
  const isDashboardRoute = /\/h\/[^/]+\/dashboard$/.test(pathname)

  return (
    <div className="flex flex-col min-h-dvh">
      {currentHostId && (
        <>
          {/* Desktop top bar */}
          <header className="hidden md:flex h-12 px-3 border-b border-border bg-surface-alt items-center justify-between gap-4 shrink-0">
            <TopbarHostMenu />
            <TopbarIconNav hostId={currentHostId} />
          </header>

          {/* Mobile top bar */}
          <TopbarMobile hostId={currentHostId} />
        </>
      )}

      <main className="flex-1 min-h-0 overflow-auto">
        <InstallPwaBanner />
        {isDashboardRoute && <PushPermissionBanner />}
        <Outlet />
      </main>
    </div>
  )
}
