interface SectionEyebrowProps {
  index: string
  label: string
}

// Small monospace label paired with a numeric index — used as the eyebrow on
// every major section to give the page an editorial, sectioned cadence.
export default function SectionEyebrow({ index, label }: SectionEyebrowProps) {
  return (
    <div className="flex items-center gap-3 text-text-secondary">
      <span className="font-mono text-[11px] tabular-nums text-text-muted tracking-[0.2em]">
        {index}
      </span>
      <span className="h-px w-8 bg-border" />
      <span className="font-mono text-[11px] uppercase tracking-[0.18em]">
        {label}
      </span>
    </div>
  )
}
