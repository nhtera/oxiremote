import type { ReactNode } from 'react'

interface SkeletonLineProps {
  className?: string
  width?: string
}

// Single shimmer rectangle. Width defaults to full; height inherits from line-box
// so it can sit inline as a placeholder for text. Use for inline runs like
// "Connected: ___" while waiting for the value.
export function SkeletonLine({ className, width }: SkeletonLineProps) {
  const cls = [
    'inline-block h-3 align-middle rounded bg-surface-hover/70 animate-pulse',
    width ? '' : 'w-full',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ')
  const style = width ? { width } : undefined
  return <span aria-hidden="true" className={cls} style={style} />
}

interface SkeletonCardProps {
  /** Title slot — defaults to a shimmer line. Pass null to omit. */
  title?: ReactNode
  /** Number of body lines to render. */
  lines?: number
  className?: string
}

// Card-shaped skeleton matching the surface-alt + border-border + rounded-lg
// look the dashboard cards use, so swap-in at first paint is visually quiet.
export function SkeletonCard({ title, lines = 3, className }: SkeletonCardProps) {
  const cls = [
    'rounded-lg border border-border bg-surface-alt p-4 space-y-3',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <div role="status" aria-busy="true" aria-label="Loading" className={cls}>
      {title === undefined ? (
        <SkeletonLine width="40%" className="h-4" />
      ) : title === null ? null : (
        title
      )}
      <div className="space-y-2 pt-1">
        {Array.from({ length: lines }).map((_, i) => (
          <SkeletonLine
            key={i}
            width={i === lines - 1 ? '60%' : '100%'}
            className="h-3"
          />
        ))}
      </div>
    </div>
  )
}
