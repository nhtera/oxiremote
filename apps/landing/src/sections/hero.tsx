import CtaButton from '../components/cta-button'
import CopyCommand from '../components/copy-command'
import HeroTerminal from './hero-terminal'
import HeroHandset from './hero-handset'

export default function Hero() {
  return (
    <section className="relative overflow-hidden">
      {/* Hairline grid backdrop fades around the hero */}
      <div className="absolute inset-0 bg-grid pointer-events-none" />

      <div className="relative mx-auto max-w-7xl px-5 sm:px-8 pt-28 pb-20 sm:pt-36 sm:pb-28">
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-12 lg:gap-8 items-center">
          {/* Headline column */}
          <div className="lg:col-span-7 reveal">
            <a
              href="https://github.com/nhtera/oxiremote/releases"
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 rounded-full border border-border bg-surface-alt/60 backdrop-blur-sm pl-1 pr-3 py-1 text-[12px] text-text-secondary hover:border-text-secondary transition-colors"
            >
              <span className="inline-flex items-center gap-1.5 rounded-full bg-accent/15 text-accent px-2 py-0.5 text-[10.5px] font-mono uppercase tracking-wider">
                <span className="w-1.5 h-1.5 rounded-full bg-accent" />
                v0.4.2
              </span>
              <span>H.264 video pipeline · WebPush deep links</span>
              <svg viewBox="0 0 16 16" className="w-3 h-3 opacity-70" fill="none" stroke="currentColor" strokeWidth="1.6">
                <path d="M6 4l4 4-4 4" />
              </svg>
            </a>

            <h1 className="heading-display mt-6 text-[clamp(2.5rem,7.5vw,5.25rem)] text-text-primary">
              Your dev box,
              <br />
              <span className="text-coral-gradient">in your pocket.</span>
              <br />
              <em className="text-text-secondary">No cloud. No middleman.</em>
            </h1>

            <p className="mt-7 max-w-xl text-[16.5px] leading-[1.55] text-text-secondary">
              Run one binary on your laptop. Reach the terminal, files, dev-server
              previews, and full remote desktop from any phone — through your own
              Cloudflare tunnel. Self-hosted, open source, and ready in under a minute.
            </p>

            <div className="mt-9 flex flex-wrap items-center gap-3">
              <CtaButton href="#install" variant="primary">
                Install OxiRemote
              </CtaButton>
              <CtaButton
                href="https://github.com/nhtera/oxiremote"
                variant="outline"
                external
                icon={
                  <svg viewBox="0 0 24 24" className="w-4 h-4" fill="currentColor" aria-hidden>
                    <path d="M12 1a11 11 0 0 0-3.48 21.45c.55.1.75-.24.75-.53v-1.84c-3.06.66-3.7-1.48-3.7-1.48-.5-1.27-1.22-1.6-1.22-1.6-1-.69.07-.67.07-.67 1.1.07 1.69 1.13 1.69 1.13.98 1.69 2.58 1.2 3.21.92.1-.71.39-1.2.7-1.48-2.44-.28-5.01-1.22-5.01-5.42 0-1.2.43-2.18 1.13-2.95-.11-.28-.49-1.4.11-2.92 0 0 .92-.3 3.02 1.13a10.4 10.4 0 0 1 5.5 0c2.1-1.43 3.02-1.13 3.02-1.13.6 1.52.22 2.64.11 2.92.7.77 1.13 1.75 1.13 2.95 0 4.21-2.57 5.13-5.02 5.41.39.34.74 1.01.74 2.04v3.02c0 .29.2.64.76.53A11 11 0 0 0 12 1Z" />
                  </svg>
                }
              >
                Star on GitHub
              </CtaButton>
            </div>

            <div className="mt-6">
              <CopyCommand command="curl -fsSL get.oxiremote.dev | bash" />
              <p className="mt-2.5 text-[12px] text-text-muted font-mono">
                or <span className="text-text-secondary">npm i -g oxiremote</span>{' '}
                · macOS · Linux · Windows · Codespaces
              </p>
            </div>

            {/* Stats strip */}
            <dl className="mt-10 grid grid-cols-4 gap-2 sm:gap-6 max-w-xl">
              {[
                { k: '<60s', v: 'first connect' },
                { k: '38ms', v: 'p50 latency' },
                { k: 'Zero', v: 'config files' },
                { k: 'MIT', v: 'open source' },
              ].map((s) => (
                <div key={s.v} className="flex flex-col gap-0.5">
                  <dt className="font-sans text-[clamp(1.1rem,2.6vw,1.6rem)] font-semibold text-text-primary tabular-nums tracking-tight">
                    {s.k}
                  </dt>
                  <dd className="text-[11px] uppercase tracking-[0.12em] text-text-muted">
                    {s.v}
                  </dd>
                </div>
              ))}
            </dl>
          </div>

          {/* Visual column — terminal stack + phone */}
          <div className="lg:col-span-5 relative reveal" style={{ ['--reveal-delay' as string]: '120ms' }}>
            <div className="relative grid grid-cols-1 gap-6">
              <HeroTerminal />
              <div className="relative h-[500px] sm:h-[560px] hidden md:block">
                <div className="absolute inset-x-0 -top-16 flex justify-center">
                  <HeroHandset />
                </div>
              </div>
              <div className="md:hidden mt-2">
                <HeroHandset />
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Bottom fade into the next section */}
      <div className="pointer-events-none absolute inset-x-0 bottom-0 h-24 bg-gradient-to-b from-transparent to-surface" />
    </section>
  )
}
