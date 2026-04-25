import { useState } from 'react'

// Header-mounted switch for the `auto_approve` setting. POSTs to
// /api/agent/settings/auto-approve and assumes the parent already pulled the
// initial value via /api/agent/state.

interface Props {
  enabled: boolean
  onChange: (next: boolean) => void
}

export default function AutoApproveToggle({ enabled, onChange }: Props) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const toggle = async () => {
    const next = !enabled
    setBusy(true)
    setError(null)
    try {
      const res = await fetch('/api/agent/settings/auto-approve', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: next }),
      })
      if (!res.ok) {
        setError(`Toggle failed (${res.status})`)
        return
      }
      onChange(next)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Toggle failed')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        role="switch"
        aria-checked={enabled}
        disabled={busy}
        onClick={toggle}
        className={`relative inline-flex h-5 w-9 items-center rounded-full border transition-colors disabled:opacity-50 ${
          enabled
            ? 'bg-accent/30 border-accent/60'
            : 'bg-surface-alt border-border'
        }`}
        title={enabled ? 'Auto-approve: ON' : 'Auto-approve: OFF'}
      >
        <span
          className={`inline-block h-3 w-3 transform rounded-full transition-transform ${
            enabled ? 'translate-x-5 bg-accent' : 'translate-x-1 bg-text-muted'
          }`}
        />
      </button>
      <span className="text-xs text-text-muted">
        Auto-approve <span className="text-text-primary">{enabled ? 'on' : 'off'}</span>
      </span>
      {error && (
        <span className="text-xs text-danger" title={error}>
          !
        </span>
      )}
    </div>
  )
}
