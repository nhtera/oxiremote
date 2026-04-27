interface Props {
  value: string
  onChange: (s: string) => void
}

export default function DeviceLabelField({ value, onChange }: Props) {
  return (
    <details className="mt-4 group">
      <summary className="cursor-pointer text-xs text-text-muted hover:text-text-secondary list-none flex items-center gap-1 select-none">
        <svg
          viewBox="0 0 24 24"
          className="w-3 h-3 transition-transform group-open:rotate-90"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <polyline points="9 18 15 12 9 6" />
        </svg>
        Name this device (optional)
      </summary>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="iPhone 15 Pro"
        maxLength={80}
        className="w-full mt-2 px-3 py-2.5 text-sm bg-surface-alt border border-border rounded-lg text-text-primary focus:outline-none focus:border-accent/50"
      />
      <p className="mt-1.5 text-xs text-text-muted">
        Helps you identify this device in the host's device list later.
      </p>
    </details>
  )
}
