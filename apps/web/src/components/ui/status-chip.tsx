import type { ReactNode } from 'react'

export type StatusVariant =
  | 'online'
  | 'offline'
  | 'pending'
  | 'rejected'
  | 'info'
  | 'warning'

interface Props {
  variant: StatusVariant
  children: ReactNode
  /** Hide the leading dot — use when the chip itself is decorative. */
  noDot?: boolean
  className?: string
  title?: string
}

interface VariantStyle {
  pill: string
  dot: string
  pulse?: boolean
}

const STYLES: Record<StatusVariant, VariantStyle> = {
  online: {
    pill: 'bg-success/10 text-success border-success/30',
    dot: 'bg-success',
  },
  offline: {
    pill: 'bg-surface-hover text-text-muted border-border',
    dot: 'bg-text-muted',
  },
  pending: {
    pill: 'bg-warning/10 text-warning border-warning/30',
    dot: 'bg-warning',
    pulse: true,
  },
  rejected: {
    pill: 'bg-danger/10 text-danger border-danger/30',
    dot: 'bg-danger',
  },
  info: {
    pill: 'bg-accent/10 text-accent border-accent/30',
    dot: 'bg-accent',
  },
  warning: {
    pill: 'bg-warning/10 text-warning border-warning/30',
    dot: 'bg-warning',
  },
}

// Pill chip with semantic variants. Replaces ad-hoc bg-success/10 spans
// scattered across devices/tunnel/health components — single source of truth
// so a tone tweak lands everywhere at once.
export default function StatusChip({
  variant,
  children,
  noDot,
  className,
  title,
}: Props) {
  const v = STYLES[variant]
  const cls = [
    'inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full border text-[length:var(--text-meta)] font-medium leading-none',
    v.pill,
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <span className={cls} title={title}>
      {noDot ? null : (
        <span
          aria-hidden="true"
          className={[
            'h-1.5 w-1.5 rounded-full',
            v.dot,
            v.pulse ? 'animate-pulse' : '',
          ]
            .filter(Boolean)
            .join(' ')}
        />
      )}
      {children}
    </span>
  )
}
