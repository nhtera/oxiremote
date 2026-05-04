import { useEffect, useState } from 'react'
import BrandMark from '../components/brand-mark'

const NAV_LINKS = [
  { label: 'Features', href: '#features' },
  { label: 'How it works', href: '#how-it-works' },
  { label: 'Compare', href: '#compare' },
  { label: 'Install', href: '#install' },
]

const VERSION = 'v0.4.2'

export default function SiteNav() {
  const [scrolled, setScrolled] = useState(false)
  const [mobileOpen, setMobileOpen] = useState(false)

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 16)
    onScroll()
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => window.removeEventListener('scroll', onScroll)
  }, [])

  return (
    <header
      className={`fixed inset-x-0 top-0 z-50 transition-all duration-300 ${
        scrolled
          ? 'border-b border-border/70 bg-surface/70 backdrop-blur-xl backdrop-saturate-150'
          : 'border-b border-transparent'
      }`}
    >
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="flex h-16 items-center justify-between">
          <a href="#" className="flex items-center gap-2.5">
            <BrandMark size={32} />
            <span className="font-semibold text-[15px] tracking-tight">
              OxiRemote
            </span>
            <span className="hidden sm:inline-block font-mono text-[10px] tracking-wide text-text-muted bg-surface-alt border border-border rounded px-1.5 py-0.5">
              {VERSION}
            </span>
          </a>

          <nav className="hidden md:flex items-center gap-1">
            {NAV_LINKS.map((l) => (
              <a
                key={l.href}
                href={l.href}
                className="px-3 py-2 text-[13.5px] text-text-secondary hover:text-text-primary transition-colors rounded-md"
              >
                {l.label}
              </a>
            ))}
            <a
              href="https://github.com/nhtera/oxiremote"
              target="_blank"
              rel="noreferrer"
              className="px-3 py-2 text-[13.5px] text-text-secondary hover:text-text-primary transition-colors rounded-md"
            >
              GitHub
            </a>
          </nav>

          <div className="flex items-center gap-2">
            <a
              href="https://github.com/nhtera/oxiremote"
              target="_blank"
              rel="noreferrer"
              aria-label="GitHub repository"
              className="hidden sm:inline-flex items-center justify-center w-9 h-9 rounded-lg border border-border text-text-secondary hover:text-text-primary hover:border-text-secondary transition-colors"
            >
              <svg viewBox="0 0 24 24" className="w-4 h-4" fill="currentColor" aria-hidden>
                <path d="M12 1a11 11 0 0 0-3.48 21.45c.55.1.75-.24.75-.53v-1.84c-3.06.66-3.7-1.48-3.7-1.48-.5-1.27-1.22-1.6-1.22-1.6-1-.69.07-.67.07-.67 1.1.07 1.69 1.13 1.69 1.13.98 1.69 2.58 1.2 3.21.92.1-.71.39-1.2.7-1.48-2.44-.28-5.01-1.22-5.01-5.42 0-1.2.43-2.18 1.13-2.95-.11-.28-.49-1.4.11-2.92 0 0 .92-.3 3.02 1.13a10.4 10.4 0 0 1 5.5 0c2.1-1.43 3.02-1.13 3.02-1.13.6 1.52.22 2.64.11 2.92.7.77 1.13 1.75 1.13 2.95 0 4.21-2.57 5.13-5.02 5.41.39.34.74 1.01.74 2.04v3.02c0 .29.2.64.76.53A11 11 0 0 0 12 1Z" />
              </svg>
            </a>
            <a
              href="#install"
              className="hidden sm:inline-flex items-center gap-1.5 h-9 rounded-lg bg-accent px-3.5 text-[13.5px] font-medium text-white ring-accent-soft hover:bg-accent-soft transition-colors"
            >
              Install
              <svg viewBox="0 0 16 16" className="w-3 h-3" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
                <path d="M3 8h10M9 4l4 4-4 4" />
              </svg>
            </a>
            <button
              type="button"
              onClick={() => setMobileOpen((v) => !v)}
              aria-label="Toggle menu"
              className="md:hidden inline-flex items-center justify-center w-9 h-9 rounded-lg border border-border text-text-secondary"
            >
              <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                {mobileOpen ? <path d="M6 6l12 12M6 18L18 6" /> : <><path d="M4 7h16" /><path d="M4 17h16" /></>}
              </svg>
            </button>
          </div>
        </div>

        {mobileOpen && (
          <div className="md:hidden pb-4 flex flex-col gap-1">
            {NAV_LINKS.map((l) => (
              <a
                key={l.href}
                href={l.href}
                onClick={() => setMobileOpen(false)}
                className="px-3 py-2.5 text-[14px] text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover"
              >
                {l.label}
              </a>
            ))}
            <a
              href="https://github.com/nhtera/oxiremote"
              target="_blank"
              rel="noreferrer"
              className="px-3 py-2.5 text-[14px] text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover"
            >
              GitHub
            </a>
          </div>
        )}
      </div>
    </header>
  )
}
