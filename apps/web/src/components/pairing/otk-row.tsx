import { useMemo, useState } from 'react'
import { CopyIcon } from '../icons'

interface Props {
  formattedKey: string
  rawKey: string | null
  remaining: number
  expired: boolean
  expiringSoon: boolean
  hasKey: boolean
  otkActive: boolean
  /** True after the SSE `otk_used` event arrives. The token is masked and
   *  the countdown swaps for a "Used" pill — matches 9remote's pairing flow
   *  where a consumed code visibly retreats. */
  consumed?: boolean
  /** Called when the user clicks "Generate new OTK" from the expiry CTA. */
  onRegenerate?: () => void
}

export default function OtkRow({
  formattedKey,
  rawKey,
  remaining,
  expired,
  expiringSoon,
  hasKey,
  otkActive,
  consumed = false,
  onRegenerate,
}: Props) {
  const [copied, setCopied] = useState(false)

  const handleCopy = async () => {
    if (!rawKey) return
    try {
      await navigator.clipboard.writeText(rawKey)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // non-secure context — silently ignore.
    }
  }

  // Bullet-mask the token while consumed. Same group spacing as the live key
  // so the row doesn't reflow when the SSE `otk_used` event arrives.
  const maskedKey = useMemo(
    () =>
      formattedKey
        ? formattedKey
            .split(' ')
            .map((g) => '•'.repeat(g.length))
            .join(' ')
        : '—',
    [formattedKey],
  )
  const displayKey = consumed ? maskedKey : (formattedKey || '—')

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-text-muted">One-time key</span>
        <span className="text-[10px] text-text-muted">Single use · expires</span>
      </div>
      <div
        className={
          'flex items-center gap-2 px-3 py-2.5 bg-surface-alt border rounded-lg ' +
          (otkActive ? 'border-border' : 'border-border/60')
        }
      >
        <ClockIcon
          className={
            otkActive
              ? 'text-accent'
              : consumed
                ? 'text-success'
                : 'text-text-muted'
          }
        />
        <code
          className={
            'flex-1 min-w-0 font-mono text-sm tracking-[0.08em] truncate select-all ' +
            (otkActive
              ? 'text-text-primary'
              : consumed
                ? 'text-text-muted'
                : 'text-text-muted line-through')
          }
          aria-label={consumed ? 'One-time key (consumed)' : 'One-time key'}
        >
          {displayKey}
        </code>
        {expired && hasKey && (
          <span className="shrink-0 text-[10px] font-semibold uppercase tracking-wider text-danger bg-danger/10 border border-danger/30 rounded px-1.5 py-0.5">
            Expired
          </span>
        )}
        <button
          onClick={handleCopy}
          disabled={!otkActive || !rawKey}
          aria-label={copied ? 'Copied to clipboard' : 'Copy one-time key'}
          className={
            'shrink-0 inline-flex items-center justify-center w-7 h-7 rounded transition-colors disabled:opacity-30 disabled:cursor-not-allowed ' +
            (copied
              ? 'text-success bg-success/15'
              : 'text-text-secondary hover:text-text-primary hover:bg-surface-hover')
          }
          title={copied ? 'Copied' : 'Copy'}
        >
          {copied ? <CheckIcon /> : <CopyIcon size={14} />}
        </button>
      </div>

      <Countdown
        remaining={remaining}
        expired={expired}
        expiringSoon={expiringSoon}
        hasKey={hasKey}
        consumed={consumed}
        onRegenerate={onRegenerate}
      />
    </div>
  )
}

function ClockIcon({ className = '' }: { className?: string }) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`shrink-0 ${className}`}
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </svg>
  )
}

function CheckIcon() {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2.25}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polyline points="5 12 10 17 19 7" />
    </svg>
  )
}

interface CountdownProps {
  remaining: number
  expired: boolean
  expiringSoon: boolean
  hasKey: boolean
  consumed?: boolean
  onRegenerate?: () => void
}

function Countdown({ remaining, expired, expiringSoon, hasKey, consumed = false, onRegenerate }: CountdownProps) {
  if (!hasKey) {
    return (
      <div className="text-xs text-text-muted">
        No active key. Tap{' '}
        <span className="text-text-secondary font-medium">New key</span> to
        generate one.
      </div>
    )
  }
  if (consumed) {
    // Distinct from time-expiry: green tone signals "the pair flow worked".
    return (
      <div className="flex items-center gap-2 flex-wrap">
        <span className="inline-flex items-center gap-1.5 text-xs font-medium text-success bg-success/10 border border-success/30 rounded-full px-2.5 py-1">
          <span className="w-2 h-2 rounded-full bg-success" />
          Used — pairing complete
        </span>
        {onRegenerate && (
          <button
            type="button"
            onClick={onRegenerate}
            className="text-xs font-medium text-accent hover:text-accent-hover transition-colors"
          >
            Generate new OTK →
          </button>
        )}
      </div>
    )
  }
  if (expired) {
    return (
      <div className="flex items-center gap-2 flex-wrap">
        <span className="inline-flex items-center gap-1.5 text-xs font-medium text-text-muted bg-surface-alt border border-border rounded-full px-2.5 py-1">
          <span className="w-2 h-2 rounded-full bg-text-muted" />
          Expired — generate a new OTK
        </span>
        {onRegenerate && (
          <button
            type="button"
            onClick={onRegenerate}
            className="text-xs font-medium text-accent hover:text-accent-hover transition-colors"
          >
            Generate new OTK →
          </button>
        )}
      </div>
    )
  }
  const m = Math.floor(remaining / 60)
  const s = remaining % 60
  const tone = expiringSoon
    ? 'text-warning bg-warning/10 border-warning/30'
    : 'text-text-secondary bg-surface-alt border-border'
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-xs font-medium rounded-full px-2.5 py-1 border ${tone}`}
    >
      <span
        className={
          'w-2 h-2 rounded-full ' + (expiringSoon ? 'bg-warning' : 'bg-success')
        }
      />
      Expires in {m}:{s.toString().padStart(2, '0')}
    </span>
  )
}
