// Presentation primitives for a single tunnel-progress row: the icon (done /
// active spinner / failed X / pending dot) and the trailing badge.

import type { StepStatus } from './step-types'

export function StepIcon({ status }: { status: StepStatus }) {
  const base =
    'inline-flex w-7 h-7 items-center justify-center rounded-full shrink-0 transition-colors'
  if (status === 'done') {
    return (
      <span className={`${base} bg-accent text-white shadow-[0_0_0_3px_rgba(255,122,64,0.10)]`}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" className="w-3.5 h-3.5">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </span>
    )
  }
  if (status === 'active') {
    return (
      <span className={`${base} bg-accent/10 border border-accent/40 text-accent`}>
        <span className="inline-block w-2 h-2 rounded-full bg-accent animate-ping absolute" />
        <span className="inline-block w-2 h-2 rounded-full bg-accent" />
      </span>
    )
  }
  if (status === 'failed') {
    return (
      <span className={`${base} bg-danger/15 border border-danger/40 text-danger`}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" className="w-3.5 h-3.5">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </span>
    )
  }
  return (
    <span className={`${base} border border-border bg-surface-alt`}>
      <span className="inline-block w-1.5 h-1.5 rounded-full bg-text-muted/60" />
    </span>
  )
}

export function StatusBadge({ status }: { status: StepStatus }) {
  if (status === 'done') {
    return (
      <span className="text-[11px] font-medium tracking-wide text-text-muted shrink-0">Done</span>
    )
  }
  if (status === 'active') {
    return (
      <span className="inline-flex items-center gap-1.5 text-[11px] font-medium tracking-wide text-accent bg-accent/10 border border-accent/30 rounded-full px-2 py-0.5 shrink-0">
        <span className="relative inline-flex w-1.5 h-1.5">
          <span className="absolute inline-flex w-full h-full rounded-full bg-accent opacity-60 animate-ping" />
          <span className="relative inline-flex w-1.5 h-1.5 rounded-full bg-accent" />
        </span>
        Running
      </span>
    )
  }
  if (status === 'failed') {
    return <span className="text-[11px] font-medium tracking-wide text-danger shrink-0">Failed</span>
  }
  return null
}
