import { useState } from 'react'
import { CopyIcon } from '../icons'

interface Props {
  formattedKey: string
  rawKey: string | null
  remaining: number
  expired: boolean
  expiringSoon: boolean
  hasKey: boolean
  otkActive: boolean
}

export default function OtkRow({
  formattedKey,
  rawKey,
  remaining,
  expired,
  expiringSoon,
  hasKey,
  otkActive,
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

  return (
    <div className="space-y-2">
      <div className="text-xs font-medium text-text-muted">One-time key</div>
      <div
        className={
          'flex items-center justify-between gap-3 px-3 py-3 bg-surface-alt border rounded-lg ' +
          (otkActive ? 'border-border' : 'border-border/60 opacity-60')
        }
      >
        <code className="font-mono text-base md:text-lg text-text-primary tracking-[0.06em] truncate select-all">
          {formattedKey || '—'}
        </code>
        <button
          onClick={handleCopy}
          disabled={!otkActive || !rawKey}
          aria-label="Copy one-time key"
          className="shrink-0 inline-flex items-center gap-1 px-2 py-1 text-xs text-text-secondary hover:text-text-primary disabled:opacity-30 disabled:cursor-not-allowed"
        >
          <CopyIcon size={14} />
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>

      <Countdown
        remaining={remaining}
        expired={expired}
        expiringSoon={expiringSoon}
        hasKey={hasKey}
      />
    </div>
  )
}

interface CountdownProps {
  remaining: number
  expired: boolean
  expiringSoon: boolean
  hasKey: boolean
}

function Countdown({ remaining, expired, expiringSoon, hasKey }: CountdownProps) {
  if (!hasKey) {
    return (
      <div className="text-xs text-text-muted">
        No active key. Tap{' '}
        <span className="text-text-secondary font-medium">New key</span> to
        generate one.
      </div>
    )
  }
  if (expired) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-danger bg-danger/10 border border-danger/30 rounded-full px-2.5 py-1">
        <span className="w-2 h-2 rounded-full bg-danger" />
        Expired
      </span>
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
