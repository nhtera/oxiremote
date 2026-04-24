import { useEffect, useState } from 'react'

interface Props {
  token: string | null
  expiresAt: number // Unix timestamp in seconds
  onRegenerate: () => Promise<void>
}

// Displays the current one-time key with a live countdown.
// Ticks every 10s. Shows "Expires in Xm Ys" or a red "Expired" badge.
export default function OneTimeKeyField({ token, expiresAt, onRegenerate }: Props) {
  const [remaining, setRemaining] = useState<number>(() =>
    Math.floor(expiresAt - Date.now() / 1000),
  )
  const [regenerating, setRegenerating] = useState(false)

  // Recompute remaining whenever expiresAt changes or every 10s
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

  const expired = remaining <= 0
  const minutes = Math.floor(remaining / 60)
  const seconds = remaining % 60

  return (
    <div className="flex flex-col gap-2">
      {token ? (
        <div className="flex items-center gap-2 flex-wrap">
          <span className="font-mono text-sm text-text-primary tracking-widest bg-surface-alt border border-border rounded-md px-3 py-1.5 select-all">
            {token}
          </span>
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
        <span className="text-sm text-text-muted">No active key</span>
      )}

      <button
        onClick={handleRegen}
        disabled={regenerating}
        className="self-start px-3 py-1.5 text-xs font-medium bg-warning/15 text-warning border border-warning/30 rounded-md hover:bg-warning/25 transition-colors disabled:opacity-40"
      >
        {regenerating ? 'Generating…' : 'Regenerate Key'}
      </button>
    </div>
  )
}
