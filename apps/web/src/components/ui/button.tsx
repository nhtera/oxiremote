import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { forwardRef } from 'react'

type Variant = 'primary' | 'secondary' | 'danger' | 'ghost' | 'accent-primary'
type Size = 'sm' | 'md' | 'lg'

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children'> {
  variant?: Variant
  size?: Size
  loading?: boolean
  leftIcon?: ReactNode
  children?: ReactNode
}

const BASE =
  'inline-flex items-center justify-center gap-1.5 font-medium leading-none ' +
  'border transition-colors select-none cursor-pointer ' +
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ' +
  'disabled:cursor-not-allowed disabled:opacity-50'

const SIZE: Record<Size, string> = {
  sm: 'h-[var(--btn-h-sm)] px-2.5 text-xs',
  md: 'h-[var(--btn-h-md)] px-3 text-sm',
  lg: 'h-[var(--btn-h-lg)] px-4 text-sm',
}

const VARIANT: Record<Variant, string> = {
  // existing blue accent — most clickable actions
  primary:
    'bg-accent/15 text-accent border-accent/30 hover:bg-accent/25 hover:text-accent-hover',
  secondary:
    'bg-surface-alt text-text-secondary border-border hover:bg-surface-hover hover:text-text-primary',
  danger:
    'bg-danger/15 text-danger border-danger/30 hover:bg-danger/25',
  ghost:
    'bg-transparent text-text-secondary border-transparent hover:bg-surface-hover hover:text-text-primary',
  // brand orange — high-emphasis primary action
  'accent-primary':
    'bg-[hsl(var(--accent-primary)/0.18)] text-[hsl(var(--accent-primary))] ' +
    'border-[hsl(var(--accent-primary)/0.4)] hover:bg-[hsl(var(--accent-primary)/0.28)]',
}

function Spinner() {
  return (
    <span
      aria-hidden="true"
      className="inline-block w-3.5 h-3.5 rounded-full border-2 border-current border-t-transparent animate-spin"
    />
  )
}

const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', loading, leftIcon, disabled, className, children, ...rest },
  ref,
) {
  const cls = [
    BASE,
    SIZE[size],
    VARIANT[variant],
    'rounded-[var(--btn-radius)]',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <button
      ref={ref}
      type={rest.type ?? 'button'}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={cls}
      {...rest}
    >
      {loading ? <Spinner /> : leftIcon}
      {children}
    </button>
  )
})

export default Button
