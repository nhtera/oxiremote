interface Props {
  tab: 'connection' | 'logs'
  onChange: (next: 'connection' | 'logs') => void
}

// Coral-underline tab switcher for the agent home's right column. Local state
// only — no router change, so deep links to /agent still land on Connection.
export default function ConnLogsTabs({ tab, onChange }: Props) {
  return (
    <div role="tablist" className="flex items-center gap-6 border-b border-border">
      {(['connection', 'logs'] as const).map((t) => {
        const active = tab === t
        return (
          <button
            key={t}
            role="tab"
            aria-selected={active}
            onClick={() => onChange(t)}
            className={
              'pb-2 -mb-px text-sm font-medium transition-colors border-b-2 ' +
              (active
                ? 'text-accent border-accent'
                : 'text-text-muted border-transparent hover:text-text-secondary')
            }
          >
            {t === 'connection' ? 'Connection' : 'Logs'}
          </button>
        )
      })}
    </div>
  )
}
