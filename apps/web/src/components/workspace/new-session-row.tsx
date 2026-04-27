import { useState, type KeyboardEvent } from 'react'
import Button from '../ui/button'

type Props = {
  onCreate: (name?: string) => void | Promise<void>
  disabled?: boolean
  /** Compact variant drops the helper hint and tightens spacing for use above
   *  the tab bar. Default (false) is the prominent empty-state row. */
  compact?: boolean
}

export default function NewSessionRow({ onCreate, disabled, compact }: Props) {
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit() {
    if (busy || disabled) return
    setBusy(true)
    try {
      const trimmed = name.trim()
      await onCreate(trimmed.length > 0 ? trimmed : undefined)
      setName('')
    } finally {
      setBusy(false)
    }
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter') {
      e.preventDefault()
      void submit()
    }
  }

  return (
    <div className={compact ? 'flex items-center gap-2' : 'flex items-center gap-2 w-full max-w-md mx-auto'}>
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={onKey}
        placeholder="Terminal name (optional)"
        maxLength={64}
        disabled={busy || disabled}
        className="flex-1 min-w-0 bg-surface border border-border rounded-md px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-muted outline-none focus:border-accent/60"
        aria-label="New terminal name"
      />
      <Button
        variant="accent-glow"
        size={compact ? 'sm' : 'md'}
        onClick={() => void submit()}
        loading={busy}
        disabled={disabled}
      >
        + New
      </Button>
    </div>
  )
}
