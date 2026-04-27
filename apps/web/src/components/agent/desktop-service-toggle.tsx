import { useState } from 'react'

interface Props {
  enabled: boolean
  onChange: (next: boolean) => void
}

// Toggles the `desktop_enabled` setting via /api/agent/services/desktop. When
// off, the WS upgrade returns 503 — live sessions keep running until they drop.
export default function DesktopServiceToggle({ enabled, onChange }: Props) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const toggle = async () => {
    const next = !enabled
    setBusy(true)
    setError(null)
    try {
      const res = await fetch('/api/agent/services/desktop', {
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
          enabled ? 'bg-accent/30 border-accent/60' : 'bg-surface-alt border-border'
        }`}
        title={enabled ? 'Remote desktop: ON' : 'Remote desktop: OFF'}
      >
        <span
          className={`inline-block h-3 w-3 transform rounded-full transition-transform ${
            enabled ? 'translate-x-5 bg-accent' : 'translate-x-1 bg-text-muted'
          }`}
        />
      </button>
      <span className="text-xs font-medium text-text-secondary">
        {enabled ? 'On' : 'Off'}
      </span>
      {error && (
        <span className="text-xs text-danger" title={error}>!</span>
      )}
    </div>
  )
}
