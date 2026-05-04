interface BrandMarkProps {
  size?: number
  withWordmark?: boolean
  className?: string
}

// Canonical brand mark — orange rounded-square with the white monitor outline
// glyph used across the host dashboard, tray, and PWA icon.
export default function BrandMark({ size = 36, withWordmark = false, className = '' }: BrandMarkProps) {
  return (
    <span className={`inline-flex items-center gap-2.5 ${className}`}>
      <span
        aria-hidden
        className="relative inline-flex items-center justify-center rounded-[10px] bg-accent text-white shrink-0 ring-accent-soft"
        style={{ width: size, height: size }}
      >
        <svg
          viewBox="0 0 24 24"
          width={size * 0.5}
          height={size * 0.5}
          fill="none"
          stroke="currentColor"
          strokeWidth="2.25"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="3" y="4" width="18" height="13" rx="2" />
          <path d="M8 20h8" />
          <path d="M12 17v3" />
        </svg>
      </span>
      {withWordmark && (
        <span className="font-semibold tracking-tight text-text-primary">
          Oxi<span className="text-text-secondary font-medium">Remote</span>
        </span>
      )}
    </span>
  )
}
