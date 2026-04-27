import { useEffect, useState } from 'react'
import { CopyIcon, EyeIcon, EyeOffIcon, RefreshIcon } from './icons'

interface Props {
  token: string | null
  expiresAt: number // Unix timestamp in seconds
  onRegenerate: () => Promise<void>
}

// Reveals the OTK by default (operator's machine, screen-share is the unusual
// case). Bold coral mono uppercase for arms-length QR-side legibility. Eye
// toggle masks the code for streaming/recording sessions.
export default function OneTimeKeyField({ token, expiresAt, onRegenerate }: Props) {
  const [remaining, setRemaining] = useState<number>(() =>
    Math.floor(expiresAt - Date.now() / 1000),
  )
  const [regenerating, setRegenerating] = useState(false)
  const [hidden, setHidden] = useState(false)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    const compute = () => setRemaining(Math.floor(expiresAt - Date.now() / 1000))
    compute()
    const id = setInterval(compute, 10_000)
    return () => clearInterval(id)
  }, [expiresAt])

  const handleRegen = async () => {
    setRegenerating(true)
    try {
      await onRegenerate()
    } finally {
      setRegenerating(false)
    }
  }

  const handleCopy = async () => {
    if (!token) return
    try {
      await navigator.clipboard.writeText(token)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      // Non-secure context: clipboard unavailable; the code is still visible.
    }
  }

  const expired = remaining <= 0
  const minutes = Math.floor(remaining / 60)
  const seconds = remaining % 60
  const display = token ? (hidden ? '•'.repeat(token.length) : token.toUpperCase()) : null

  return (
    <div className="flex flex-col gap-2">
      {token ? (
        <div className="flex items-center gap-2 flex-wrap">
          <span className="font-mono text-2xl font-bold text-accent tracking-[0.2em] bg-surface-alt border border-accent/30 rounded-md px-3 py-1.5 select-all">
            {display}
          </span>
          <button
            type="button"
            onClick={() => setHidden((h) => !h)}
            className="inline-flex items-center justify-center w-8 h-8 rounded-md border border-border text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
            aria-label={hidden ? 'Show key' : 'Hide key'}
            title={hidden ? 'Show key' : 'Hide key (screen-share)'}
          >
            {hidden ? <EyeIcon size={16} /> : <EyeOffIcon size={16} />}
          </button>
          <button
            type="button"
            onClick={handleCopy}
            className="inline-flex items-center justify-center w-8 h-8 rounded-md border border-border text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
            aria-label="Copy key"
            title={copied ? 'Copied' : 'Copy key'}
          >
            <CopyIcon size={16} />
          </button>
          <button
            type="button"
            onClick={handleRegen}
            disabled={regenerating}
            className="inline-flex items-center justify-center w-8 h-8 rounded-md border border-border text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors disabled:opacity-40"
            aria-label="Regenerate key"
            title="Regenerate key"
          >
            <RefreshIcon size={16} />
          </button>
          {expired ? (
            <span className="text-xs font-medium text-danger bg-danger/10 border border-danger/30 rounded px-2 py-0.5">
              Expired
            </span>
          ) : (
            <span className="text-xs text-text-muted">
              Expires in {minutes}m {seconds}s
            </span>
          )}
        </div>
      ) : (
        <button
          onClick={handleRegen}
          disabled={regenerating}
          className="self-start px-3 py-1.5 text-xs font-medium bg-accent/15 text-accent border border-accent/30 rounded-md hover:bg-accent/25 transition-colors disabled:opacity-40"
        >
          {regenerating ? 'Generating…' : 'Generate Key'}
        </button>
      )}
    </div>
  )
}
