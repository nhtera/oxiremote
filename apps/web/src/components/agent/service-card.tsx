import type { ReactNode } from 'react'

interface Props {
  icon: ReactNode
  title: string
  subtitle: string
  /** Trailing slot — status badge, toggle, or action button. */
  trailing: ReactNode
  /** Coral-tinted background when the service is the operator's primary lever. */
  emphasized?: boolean
}

// Single row for the agent home's service grid. Used twice: terminal (always
// on, badge) and desktop (operator toggle).
export default function ServiceCard({ icon, title, subtitle, trailing, emphasized }: Props) {
  return (
    <div
      className={
        'flex items-center gap-3 rounded-xl border p-4 ' +
        (emphasized
          ? 'border-accent/30 bg-accent/[0.04]'
          : 'border-border bg-surface')
      }
    >
      <span
        className={
          'inline-flex w-10 h-10 items-center justify-center rounded-lg shrink-0 ' +
          (emphasized
            ? 'bg-accent/15 text-accent border border-accent/30'
            : 'bg-surface-alt text-text-secondary border border-border')
        }
        aria-hidden
      >
        {icon}
      </span>
      <div className="flex-1 min-w-0">
        <div className="text-sm font-semibold text-text-primary leading-tight">{title}</div>
        <div className="text-xs text-text-muted leading-snug truncate">{subtitle}</div>
      </div>
      <div className="shrink-0">{trailing}</div>
    </div>
  )
}
