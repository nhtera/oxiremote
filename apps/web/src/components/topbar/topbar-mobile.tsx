import TopbarHostMenu from './topbar-host-menu'
import TopbarIconNav from './topbar-icon-nav'

interface Props {
  hostId: string
  onOpenGear?: () => void
}

// Mobile top bar — same primitives as desktop but compacted to fit a 375px
// viewport. The icon strip horizontally scrolls if the active surface count
// grows beyond what fits, instead of being hidden behind a "more" sheet.
// Visible up to lg (1024px) to match the bottom tab bar breakpoint.
export default function TopbarMobile({ hostId, onOpenGear }: Props) {
  return (
    <div className="lg:hidden pt-safe px-safe border-b border-border bg-surface-alt shrink-0">
      <div className="h-11 px-2 flex items-center justify-between gap-2">
      <TopbarHostMenu />
      <div className="flex-1 min-w-0 overflow-x-auto">
        <TopbarIconNav hostId={hostId} compact onOpenGear={onOpenGear} />
      </div>
      </div>
    </div>
  )
}
