import CtaButton from '../components/cta-button'
import CopyCommand from '../components/copy-command'
import BrandMark from '../components/brand-mark'

export default function FinalCta() {
  return (
    <section className="relative py-24 sm:py-32">
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="reveal relative overflow-hidden rounded-[28px] border border-accent/30 ring-glass bg-gradient-to-br from-accent/15 via-surface-alt to-surface-alt p-8 sm:p-16">
          {/* Decorative orbs */}
          <div className="absolute -top-32 -right-24 w-72 h-72 rounded-full bg-accent/35 blur-[110px] pointer-events-none" />
          <div className="absolute -bottom-24 -left-16 w-72 h-72 rounded-full bg-accent-deep/30 blur-[120px] pointer-events-none" />
          <div className="absolute inset-0 bg-grid opacity-50 pointer-events-none" />

          <div className="relative grid grid-cols-1 lg:grid-cols-12 gap-10 items-center">
            <div className="lg:col-span-7 flex flex-col gap-6">
              <BrandMark size={44} />
              <h2 className="heading-display text-[clamp(2.2rem,5vw,3.8rem)] text-text-primary max-w-[18ch]">
                Code from bed.{' '}
                <span className="text-coral-gradient">Ship from a train.</span>
              </h2>
              <p className="text-[16.5px] text-text-secondary max-w-xl leading-[1.55]">
                One binary, your tunnel, your machine. Free, MIT-licensed, and
                ready in less than the time it takes to brew coffee.
              </p>
              <div className="flex flex-wrap items-center gap-3 mt-2">
                <CtaButton href="#install" variant="primary">
                  Install OxiRemote
                </CtaButton>
                <CtaButton
                  href="https://github.com/nhtera/oxiremote"
                  variant="outline"
                  external
                >
                  Browse the source
                </CtaButton>
              </div>
              <p className="font-mono text-[12px] text-text-muted">
                no credit card · no email · no telemetry · MIT license
              </p>
            </div>

            <div className="lg:col-span-5 flex flex-col gap-4">
              <CopyCommand command="curl -fsSL get.oxiremote.dev | bash" />
              <CopyCommand command="npm install -g oxiremote" />
              <div className="rounded-xl border border-border-soft bg-surface/60 backdrop-blur-sm p-4 flex items-start gap-3">
                <span className="shrink-0 w-9 h-9 rounded-lg bg-accent/15 border border-accent/30 text-accent inline-flex items-center justify-center">
                  <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83" />
                  </svg>
                </span>
                <div className="min-w-0">
                  <p className="text-[13.5px] font-medium text-text-primary">Self-update built in</p>
                  <p className="text-[12.5px] text-text-secondary leading-relaxed">
                    <code className="font-mono text-text-primary">oxiremote update</code> verifies SHA256 against the release manifest, then atomic-replaces the binary in place. Restart and you’re on latest.
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
