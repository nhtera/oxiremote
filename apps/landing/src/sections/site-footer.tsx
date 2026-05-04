import BrandMark from '../components/brand-mark'

const COLUMNS = [
  {
    title: 'Product',
    items: [
      { label: 'Features', href: '#features' },
      { label: 'How it works', href: '#how-it-works' },
      { label: 'Compare', href: '#compare' },
      { label: 'Install', href: '#install' },
      { label: 'Release notes', href: 'https://github.com/nhtera/oxiremote/releases' },
    ],
  },
  {
    title: 'Resources',
    items: [
      { label: 'Documentation', href: 'https://docs.oxiremote.dev' },
      { label: 'GitHub', href: 'https://github.com/nhtera/oxiremote' },
      { label: 'NPM package', href: 'https://www.npmjs.com/package/oxiremote' },
      { label: 'Issues & feedback', href: 'https://github.com/nhtera/oxiremote/issues' },
    ],
  },
  {
    title: 'Legal',
    items: [
      { label: 'MIT license', href: 'https://github.com/nhtera/oxiremote/blob/main/LICENSE' },
      { label: 'Security policy', href: 'https://github.com/nhtera/oxiremote/security' },
      { label: 'Privacy', href: '#' },
    ],
  },
]

export default function SiteFooter() {
  return (
    <footer className="relative border-t border-border-soft bg-surface-alt/40 backdrop-blur-sm">
      <div className="mx-auto max-w-7xl px-5 sm:px-8 pt-16 pb-10">
        <div className="grid grid-cols-2 lg:grid-cols-12 gap-10">
          <div className="col-span-2 lg:col-span-5 flex flex-col gap-5">
            <BrandMark size={40} withWordmark />
            <p className="text-[14.5px] text-text-secondary leading-[1.6] max-w-sm">
              Self-hosted remote-anywhere agent. Your dev machine, reachable from
              any phone, through your own Cloudflare tunnel.
            </p>
            <p className="font-mono text-[11.5px] text-text-muted leading-relaxed">
              Built with Rust · React 19 · TypeScript · Tailwind v4 · WebRTC.
              <br />
              No telemetry. No tracking. No third-party cookies.
            </p>
          </div>

          {COLUMNS.map((col) => (
            <div key={col.title} className="lg:col-span-2 flex flex-col gap-3">
              <h4 className="font-mono text-[11px] uppercase tracking-[0.18em] text-text-muted">
                {col.title}
              </h4>
              <ul className="flex flex-col gap-2">
                {col.items.map((item) => (
                  <li key={item.label}>
                    <a
                      href={item.href}
                      target={item.href.startsWith('http') ? '_blank' : undefined}
                      rel={item.href.startsWith('http') ? 'noreferrer' : undefined}
                      className="text-[14px] text-text-secondary hover:text-text-primary transition-colors"
                    >
                      {item.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          ))}

          <div className="lg:col-span-1 flex lg:justify-end">
            <a
              href="#"
              className="inline-flex items-center gap-1.5 h-9 px-3 rounded-full border border-border text-[12px] text-text-secondary hover:text-text-primary hover:border-text-secondary transition-colors"
              aria-label="Back to top"
            >
              <svg viewBox="0 0 16 16" className="w-3 h-3" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
                <path d="M8 13V3M3 8l5-5 5 5" />
              </svg>
              top
            </a>
          </div>
        </div>

        <div className="mt-14 pt-6 border-t border-border-soft flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4">
          <p className="font-mono text-[11.5px] text-text-muted">
            © {new Date().getFullYear()} OxiRemote · MIT License · v0.4.2
          </p>
          <div className="flex items-center gap-3">
            <span className="inline-flex items-center gap-1.5 text-[11.5px] text-text-secondary">
              <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
              all systems normal
            </span>
            <span className="text-text-muted">·</span>
            <a
              href="https://github.com/nhtera/oxiremote"
              target="_blank"
              rel="noreferrer"
              className="text-[11.5px] text-text-secondary hover:text-text-primary transition-colors"
            >
              GitHub
            </a>
          </div>
        </div>
      </div>

      {/* Massive wordmark, brand-shy */}
      <div
        aria-hidden
        className="pointer-events-none select-none overflow-hidden flex items-end justify-center -mb-[6vw]"
      >
        <span
          className="font-sans font-semibold tracking-[-0.05em] text-[28vw] leading-none"
          style={{
            background:
              'linear-gradient(180deg, color-mix(in srgb, var(--color-accent) 22%, transparent) 0%, transparent 70%)',
            WebkitBackgroundClip: 'text',
            backgroundClip: 'text',
            color: 'transparent',
          }}
        >
          OxiRemote
        </span>
      </div>
    </footer>
  )
}
