import type { ReactNode } from 'react'

interface Props {
  /** Sticky pairing rail on lg+; first card on mobile (vertical scroll). */
  rail: ReactNode
  /** Right pane: services strip + tabbed content + per-tab panels. */
  main: ReactNode
}

// Two-column shell for the agent home (lg ≥ 1024 px). Mobile collapses to a
// single vertical scroll where the rail (PairingCard) renders first, then the
// services strip + tabs.
//
// Sticky pairing rail uses `lg:sticky lg:top-0 lg:h-screen` so it stays put
// while the right pane scrolls. iOS Safari behaves correctly here as long as
// the parent grid never sets a fixed height.
export default function HostShell({ rail, main }: Props) {
  return (
    <div className="grid grid-cols-1 lg:grid-cols-[360px_1fr] min-h-full">
      <aside className="lg:border-r border-border lg:sticky lg:top-0 lg:h-screen lg:overflow-y-auto p-4 lg:p-6">
        {rail}
      </aside>
      <main className="overflow-y-auto p-4 lg:p-6 space-y-4">{main}</main>
    </div>
  )
}
