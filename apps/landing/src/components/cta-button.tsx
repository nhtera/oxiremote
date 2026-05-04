import type { ReactNode } from 'react'

type Variant = 'primary' | 'ghost' | 'outline'

interface CtaButtonProps {
  href: string
  children: ReactNode
  variant?: Variant
  icon?: ReactNode
  external?: boolean
  className?: string
}

const VARIANTS: Record<Variant, string> = {
  primary:
    'bg-accent text-white ring-accent-soft hover:bg-accent-soft active:translate-y-[1px]',
  outline:
    'bg-surface-alt/60 backdrop-blur-sm text-text-primary border border-border hover:border-text-secondary hover:bg-surface-hover',
  ghost:
    'bg-transparent text-text-secondary hover:text-text-primary',
}

export default function CtaButton({
  href,
  children,
  variant = 'primary',
  icon,
  external = false,
  className = '',
}: CtaButtonProps) {
  return (
    <a
      href={href}
      target={external ? '_blank' : undefined}
      rel={external ? 'noreferrer' : undefined}
      className={`group inline-flex items-center gap-2 rounded-xl px-4 h-11 text-[14.5px] font-medium tracking-tight transition-all duration-200 ${VARIANTS[variant]} ${className}`}
    >
      {icon}
      <span>{children}</span>
      <svg
        viewBox="0 0 16 16"
        className="w-3.5 h-3.5 -mr-0.5 opacity-70 transition-transform duration-300 group-hover:translate-x-0.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M3 8h10M9 4l4 4-4 4" />
      </svg>
    </a>
  )
}
