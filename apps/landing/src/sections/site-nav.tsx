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

  // Lock body scroll while the mobile drawer is open so the page underneath
  // doesn't drift behind the overlay. Restored on close / unmount.
  useEffect(() => {
    if (!mobileOpen) return
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMobileOpen(false)
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.body.style.overflow = prev
      document.removeEventListener('keydown', onKey)
    }
  }, [mobileOpen])

  // Header gets the opaque treatment when EITHER the page is scrolled OR the
  // mobile drawer is open — keeps menu items legible on the hero where the
  // header would otherwise be transparent.
  const headerOpaque = scrolled || mobileOpen

  return (
    <>
    <header
      className={`fixed inset-x-0 top-0 z-50 transition-all duration-300 ${
        headerOpaque
          ? 'border-b border-border/70 bg-surface/85 backdrop-blur-xl backdrop-saturate-150'
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
              href="https://remote.erai.dev"
              target="_blank"
              rel="noreferrer"
              className="hidden md:inline-flex items-center gap-1.5 h-9 rounded-lg border border-border bg-surface-alt/60 backdrop-blur-sm px-3.5 text-[13.5px] font-medium text-text-primary hover:border-text-secondary hover:bg-surface-hover transition-colors"
            >
              <span className="inline-flex items-center gap-1.5">
                <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
                Open app
              </span>
              <svg viewBox="0 0 16 16" className="w-3 h-3 opacity-60" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
                <path d="M6 4h6v6M12 4l-7 7" />
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
              aria-label={mobileOpen ? 'Close menu' : 'Open menu'}
              aria-expanded={mobileOpen}
              aria-controls="mobile-menu"
              className="md:hidden inline-flex items-center justify-center w-9 h-9 rounded-lg border border-border text-text-secondary"
            >
              <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                {mobileOpen ? <path d="M6 6l12 12M6 18L18 6" /> : <><path d="M4 7h16" /><path d="M4 17h16" /></>}
              </svg>
            </button>
          </div>
        </div>

      </div>
    </header>

    {/* Mobile drawer — rendered as a sibling of <header> (NOT inside it).
        The header has backdrop-filter, which establishes a containing block,
        which would force `position: fixed` children to resolve against the
        header's content box instead of the viewport — collapsing the drawer
        to 1px tall. Sibling positioning avoids that trap entirely. */}
    {mobileOpen && (
      <>
        {/* Backdrop catches taps outside the panel content */}
        <button
          type="button"
          aria-label="Close menu"
          onClick={() => setMobileOpen(false)}
          className="md:hidden fixed inset-0 top-16 z-40 bg-surface/40 backdrop-blur-sm animate-fade-in"
        />
        <div
          id="mobile-menu"
          role="dialog"
          aria-modal="true"
          aria-label="Site navigation"
          className="md:hidden fixed inset-x-0 top-16 bottom-0 z-50 bg-surface border-t border-border-soft overflow-y-auto"
        >
            <nav className="flex flex-col px-4 pt-4 pb-8">
              {NAV_LINKS.map((l) => (
                <a
                  key={l.href}
                  href={l.href}
                  onClick={() => setMobileOpen(false)}
                  className="flex items-center justify-between border-b border-border-soft py-4 text-[16px] text-text-primary hover:text-accent transition-colors"
                >
                  <span>{l.label}</span>
                  <svg viewBox="0 0 16 16" className="w-3.5 h-3.5 text-text-muted" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
                    <path d="M6 4l4 4-4 4" />
                  </svg>
                </a>
              ))}

              <a
                href="https://remote.erai.dev"
                target="_blank"
                rel="noreferrer"
                onClick={() => setMobileOpen(false)}
                className="mt-6 flex items-center justify-between rounded-2xl border border-accent/30 bg-accent/10 px-4 py-4 text-[15px] font-medium text-text-primary"
              >
                <span className="inline-flex items-center gap-3">
                  <span className="inline-flex w-2 h-2 rounded-full bg-success live-dot" />
                  <span>
                    <span className="block">Open the live app</span>
                    <span className="block font-mono text-[11.5px] text-text-secondary mt-0.5">remote.erai.dev</span>
                  </span>
                </span>
                <svg viewBox="0 0 16 16" className="w-3.5 h-3.5 text-accent" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M6 4h6v6M12 4l-7 7" />
                </svg>
              </a>

              <a
                href="#install"
                onClick={() => setMobileOpen(false)}
                className="mt-3 flex items-center justify-between rounded-2xl bg-accent ring-accent-soft px-4 py-4 text-[15px] font-medium text-white"
              >
                <span>Install OxiRemote</span>
                <svg viewBox="0 0 16 16" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
                  <path d="M3 8h10M9 4l4 4-4 4" />
                </svg>
              </a>

              <div className="mt-8 flex items-center justify-between text-[12px] text-text-muted">
                <a
                  href="https://github.com/nhtera/oxiremote"
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex items-center gap-2 text-text-secondary hover:text-text-primary"
                >
                  <svg viewBox="0 0 24 24" className="w-4 h-4" fill="currentColor" aria-hidden>
                    <path d="M12 1a11 11 0 0 0-3.48 21.45c.55.1.75-.24.75-.53v-1.84c-3.06.66-3.7-1.48-3.7-1.48-.5-1.27-1.22-1.6-1.22-1.6-1-.69.07-.67.07-.67 1.1.07 1.69 1.13 1.69 1.13.98 1.69 2.58 1.2 3.21.92.1-.71.39-1.2.7-1.48-2.44-.28-5.01-1.22-5.01-5.42 0-1.2.43-2.18 1.13-2.95-.11-.28-.49-1.4.11-2.92 0 0 .92-.3 3.02 1.13a10.4 10.4 0 0 1 5.5 0c2.1-1.43 3.02-1.13 3.02-1.13.6 1.52.22 2.64.11 2.92.7.77 1.13 1.75 1.13 2.95 0 4.21-2.57 5.13-5.02 5.41.39.34.74 1.01.74 2.04v3.02c0 .29.2.64.76.53A11 11 0 0 0 12 1Z" />
                  </svg>
                  GitHub
                </a>
                <span className="font-mono">{VERSION}</span>
            </div>
          </nav>
        </div>
      </>
    )}
    </>
  )
}
