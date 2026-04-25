import type { ReactNode } from 'react'

type Tone = 'default' | 'error'

interface Props {
  /** Optional decorative icon — sized at 32px square by the wrapper. */
  icon?: ReactNode
  title: string
  /** Body copy — string or ReactNode for richer messages. */
  body?: ReactNode
  /** Optional CTA — usually a Button. */
  action?: ReactNode
  /** Visual tone. error tints the icon ring red. */
  tone?: Tone
  className?: string
}

// Empty / loading-failed / error placeholder. Centred, generous vertical padding
// so it works as a page-level state without the caller needing extra layout.
// Also used by workspace pages to convey "nothing here yet" with a CTA.
export default function StateView({
  icon,
  title,
  body,
  action,
  tone = 'default',
  className,
}: Props) {
  const cls = [
    'flex flex-col items-center justify-center text-center px-6 py-12 gap-3',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ')

  const ringCls =
    tone === 'error'
      ? 'border-danger/40 text-danger bg-danger/10'
      : 'border-border text-text-muted bg-surface-alt'

  return (
    <div role="status" className={cls}>
      {icon ? (
        <div
          aria-hidden="true"
          className={`flex h-12 w-12 items-center justify-center rounded-full border ${ringCls}`}
        >
          <div className="h-5 w-5">{icon}</div>
        </div>
      ) : null}
      <div className="text-[length:var(--text-h2)] font-medium text-text-primary">
        {title}
      </div>
      {body ? (
        <div className="max-w-sm text-[length:var(--text-body)] text-text-secondary leading-relaxed">
          {body}
        </div>
      ) : null}
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  )
}
