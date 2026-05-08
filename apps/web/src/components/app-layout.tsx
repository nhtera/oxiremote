import { useState } from 'react'
import { Outlet, useLocation } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import PushPermissionBanner from './push-permission-banner'
import InstallPwaBanner from './install-pwa-banner'
import TopbarHostMenu from './topbar/topbar-host-menu'
import TopbarIconNav from './topbar/topbar-icon-nav'
import TopbarMobile from './topbar/topbar-mobile'
import GearDrawer from './gear/gear-drawer'
import MobileBottomTabs from './topbar/mobile-bottom-tabs'
import { useAppChromeHeight } from '../lib/use-app-chrome-height'

// Workspace-centric chrome. Shell uses CSS Grid (auto/1fr/auto) so the routed
// page owns its own scroll container. iOS Safari's soft keyboard would clip
// xterm when an outer `overflow-auto` competed with the visualViewport — the
// grid avoids that. Mobile gets a bottom tab bar in the last grid row.
export default function AppLayout() {
  const { pathname } = useLocation()
  const { currentHostId } = useHostStore()
  const [gearOpen, setGearOpen] = useState(false)
  // Measures combined top+bottom chrome and publishes --app-chrome-height on body.
  useAppChromeHeight()

  // Push banner is a setup nudge, not an in-flow nag. Show it only when the
  // operator is on the dashboard surface (the calm one).
  const isDashboardRoute = /\/h\/[^/]+\/dashboard$/.test(pathname)

  // Remote-desktop session is full-immersion on mobile (matches Chrome Remote
  // Desktop's pattern). The route renders its own top strip + composer, so
  // hiding MobileBottomTabs here frees the bottom safe-area for the composer
  // and stops the tabs from peeking through above it. Exit via ← in the
  // top strip returns to the host page where the tabs reappear.
  const isDesktopRoute = /\/h\/[^/]+\/desktop$/.test(pathname)
  const gridRows = isDesktopRoute
    ? 'grid-rows-[auto_1fr]'
    : 'grid-rows-[auto_1fr_auto]'

  return (
    <div
      data-app-grid
      className={`grid ${gridRows} h-[100dvh] supports-[not(height:100dvh)]:h-screen`}
    >
      {currentHostId ? (
        <>
          {/* Desktop top bar */}
          <header className="hidden lg:flex h-12 px-3 border-b border-border bg-surface-alt items-center justify-between gap-4 shrink-0">
            <TopbarHostMenu />
            <TopbarIconNav hostId={currentHostId} onOpenGear={() => setGearOpen(true)} />
          </header>

          {/* Mobile top bar */}
          <TopbarMobile hostId={currentHostId} onOpenGear={() => setGearOpen(true)} />
        </>
      ) : (
        <div />
      )}

      <main className="min-h-0 overflow-hidden flex flex-col">
        <InstallPwaBanner />
        {isDashboardRoute && <PushPermissionBanner />}
        <div className="flex-1 min-h-0 overflow-auto">
          <Outlet />
        </div>
      </main>

      {currentHostId && !isDesktopRoute ? (
        <MobileBottomTabs hostId={currentHostId} />
      ) : null}

      {currentHostId && (
        <GearDrawer
          open={gearOpen}
          hostId={currentHostId}
          onClose={() => setGearOpen(false)}
        />
      )}
    </div>
  )
}
