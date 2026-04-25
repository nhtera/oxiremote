import type { ReactNode } from 'react'

type Level = 1 | 2 | 3

interface Props {
  level: Level
  children: ReactNode
  className?: string
  id?: string
}

// Heading scale anchored to --text-h1/h2/h3 tokens defined in index.css.
// Three sizes only — beyond this we add new visual hierarchy, not a fourth h-tag.
// Pages that need a hero/display size should use --text-display directly, not Heading.
const STYLES: Record<Level, string> = {
  1: 'text-[length:var(--text-h1)] font-semibold text-text-primary tracking-tight',
  2: 'text-[length:var(--text-h2)] font-medium text-text-primary',
  3: 'text-[length:var(--text-h3)] font-medium text-text-secondary uppercase tracking-wide',
}

export default function Heading({ level, children, className, id }: Props) {
  const Tag = (`h${level}`) as 'h1' | 'h2' | 'h3'
  const cls = className ? `${STYLES[level]} ${className}` : STYLES[level]
  return (
    <Tag id={id} className={cls}>
      {children}
    </Tag>
  )
}
