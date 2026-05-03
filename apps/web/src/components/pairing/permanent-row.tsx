import { useState } from 'react'
import { CopyIcon } from '../icons'

interface PermanentKeyMeta {
  last4: string
  created_at: number
}

interface Props {
  meta: PermanentKeyMeta | null
  /** Plaintext key — present only immediately after a rotation; cleared by onDismiss. */
  plaintext: string | null
  onRegenerate: () => Promise<void>
  onDismiss: () => void
}

export default function PermanentRow({ meta, plaintext, onRegenerate, onDismiss }: Props) {
  const [regenerating, setRegenerating] = useState(false)
  const [copied, setCopied] = useState(false)

  const handleRegenerate = async () => {
    setRegenerating(true)
    try {
      await onRegenerate()
    } finally {
      setRegenerating(false)
    }
  }

  const handleCopy = async () => {
    if (!plaintext) return
    try {
      await navigator.clipboard.writeText(plaintext)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // Non-secure context — silently ignore.
    }
  }

  const maskedDisplay = meta ? `sk-········${meta.last4}` : null

  return (
    <div>
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-xs font-medium text-text-muted">Permanent API key</span>
        <span className="text-[10px] text-text-muted">Reusable · no expiry</span>
      </div>

      {plaintext ? (
        <div className="rounded-lg border border-warning/40 bg-warning/5 p-3 space-y-2.5">
          <code className="block font-mono text-xs text-text-primary tracking-wider break-all select-all bg-surface px-2.5 py-2 rounded border border-border">
            {plaintext}
          </code>
          <div className="text-xs text-warning font-medium leading-snug">
            Only shown once. Save it now.
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleCopy}
              aria-label="Copy permanent API key"
              className="inline-flex items-center gap-1 px-2.5 py-1 text-xs font-medium bg-surface-alt border border-border text-text-secondary rounded-md hover:text-text-primary hover:bg-surface-hover transition-colors"
            >
              <CopyIcon size={14} />
              {copied ? 'Copied' : 'Copy'}
            </button>
            <button
              onClick={onDismiss}
              className="ml-auto px-2.5 py-1 text-xs font-medium bg-surface-alt border border-border text-text-secondary rounded-md hover:text-text-primary hover:bg-surface-hover transition-colors"
            >
              Done
            </button>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-2 px-3 py-2.5 bg-surface-alt border border-border rounded-lg">
          <KeyIcon className="text-text-muted" />
          <code className="flex-1 min-w-0 font-mono text-sm text-text-secondary tracking-wider truncate select-none">
            {maskedDisplay ?? 'Not yet generated'}
          </code>
          <button
            onClick={handleRegenerate}
            disabled={regenerating}
            className="shrink-0 px-2.5 py-1 text-xs font-medium bg-danger/10 text-danger border border-danger/30 rounded-md hover:bg-danger/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            title="Generate a fresh permanent key (invalidates old key)"
          >
            {regenerating ? 'Rotating…' : 'Regenerate'}
          </button>
        </div>
      )}
    </div>
  )
}

function KeyIcon({ className = '' }: { className?: string }) {
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
      <circle cx="7.5" cy="15.5" r="3" />
      <path d="M9.6 13.4l9.4-9.4" />
      <path d="M16 8l3 3" />
      <path d="M14 10l2 2" />
    </svg>
  )
}
