import { useState } from 'react'
import SectionEyebrow from '../components/section-eyebrow'
import CtaButton from '../components/cta-button'

type Tab = 'curl' | 'npm' | 'cargo' | 'codespaces'

const TABS: { id: Tab; label: string; cmd: string; note: string }[] = [
  {
    id: 'curl',
    label: 'macOS · Linux',
    cmd: 'curl -fsSL get.oxiremote.dev | bash',
    note: 'Drops oxiremote into ~/.local/bin · SHA256 verified · OXIREMOTE_INSTALL_DIR overrides target.',
  },
  {
    id: 'npm',
    label: 'Any platform',
    cmd: 'npm install -g oxiremote',
    note: 'Thin wrapper · postinstall fetches the signed prebuilt binary for your target triple.',
  },
  {
    id: 'cargo',
    label: 'From source',
    cmd: 'bun install && bun run build:release',
    note: 'Embeds the SPA via rust-embed · requires Rust toolchain + Bun.',
  },
  {
    id: 'codespaces',
    label: 'Codespaces',
    cmd: 'oxiremote --auto',
    note: 'Drop the .devcontainer/ scaffold into your repo · headless mode prints the QR to the console.',
  },
]

const BOOT = [
  { kind: 'in',  text: 'oxiremote' },
  { kind: 'log', text: '→ checking for cloudflared … missing, downloading' },
  { kind: 'log', text: '→ sha256 verified  8a3c…f2e1' },
  { kind: 'log', text: '→ binding 127.0.0.1:8787  ok' },
  { kind: 'log', text: '→ opening quick tunnel' },
  { kind: 'ok',  text: '✓ tunnel  https://atlas-zen-bay.trycloudflare.com' },
  { kind: 'log', text: '→ pairing code  3F8K · QR42  (valid 10 min)' },
  { kind: 'ok',  text: '✓ ready · scan QR or paste tunnel URL' },
]

export default function InstallBlock() {
  const [active, setActive] = useState<Tab>('curl')
  const tab = TABS.find((t) => t.id === active)!

  return (
    <section id="install" className="relative py-24 sm:py-32">
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-10 items-start">
          <div className="lg:col-span-5 reveal flex flex-col gap-6 lg:sticky lg:top-28">
            <SectionEyebrow index="04" label="Install" />
            <h2 className="heading-display text-[clamp(2rem,4.6vw,3.4rem)] text-text-primary">
              One command.{' '}
              <em className="text-text-secondary">Pick your shell.</em>
            </h2>
            <p className="text-text-secondary text-[16px] leading-[1.55]">
              Each release ships a SHA256 manifest and per-target archives for
              macOS arm64/x64, Linux arm64/x64, Windows x64. The installer
              verifies the archive before unpacking. <code className="font-mono text-text-primary text-[13.5px] bg-surface-alt border border-border rounded px-1.5 py-0.5">oxiremote update</code> handles the rest.
            </p>

            <div className="flex flex-wrap items-center gap-3 mt-2">
              <CtaButton href="https://docs.oxiremote.dev" variant="primary" external>
                Read the docs
              </CtaButton>
              <CtaButton
                href="https://github.com/nhtera/oxiremote/releases"
                variant="outline"
                external
              >
                Release notes
              </CtaButton>
            </div>

            <ul className="mt-6 space-y-3 text-[13.5px] text-text-secondary">
              {[
                'No telemetry, no account creation, no login email.',
                'Cloudflare tunnel is yours — switch to a named tunnel for stable hostnames.',
                'Self-update tracks `latest`; pin a tag with OXIREMOTE_VERSION.',
              ].map((b) => (
                <li key={b} className="flex gap-3">
                  <span className="mt-1.5 w-1.5 h-1.5 rounded-full bg-accent shrink-0" />
                  <span>{b}</span>
                </li>
              ))}
            </ul>
          </div>

          <div className="lg:col-span-7 reveal">
            <div className="rounded-3xl border border-border bg-surface-alt/80 backdrop-blur-md ring-glass overflow-hidden">
              <div className="flex items-center gap-1 px-3 pt-3 border-b border-border-soft">
                {TABS.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    onClick={() => setActive(t.id)}
                    className={`relative px-3.5 py-2.5 text-[12.5px] font-medium tracking-tight transition-colors rounded-t-md ${
                      active === t.id
                        ? 'text-text-primary'
                        : 'text-text-secondary hover:text-text-primary'
                    }`}
                  >
                    {t.label}
                    {active === t.id && (
                      <span className="absolute inset-x-2 bottom-0 h-0.5 bg-accent rounded-full" />
                    )}
                  </button>
                ))}
              </div>

              <div className="p-5 sm:p-6">
                <div className="flex items-center justify-between gap-3 rounded-xl bg-surface border border-border-soft px-4 py-3 font-mono text-[13.5px]">
                  <span className="flex items-center gap-3 min-w-0">
                    <span className="text-text-muted shrink-0">$</span>
                    <span className="truncate text-text-primary">{tab.cmd}</span>
                  </span>
                  <button
                    type="button"
                    onClick={() => navigator.clipboard?.writeText(tab.cmd)}
                    className="text-[11px] uppercase tracking-wider text-text-muted hover:text-text-primary transition-colors font-sans"
                  >
                    copy
                  </button>
                </div>
                <p className="mt-2.5 text-[12.5px] text-text-secondary leading-relaxed">
                  {tab.note}
                </p>

                <div className="mt-6 rounded-xl bg-surface border border-border-soft overflow-hidden">
                  <div className="flex items-center justify-between px-4 py-2.5 border-b border-border-soft">
                    <span className="font-mono text-[11px] text-text-muted">
                      ~/projects/atlas
                    </span>
                    <span className="inline-flex items-center gap-1.5 text-[10.5px] font-mono uppercase tracking-wider text-success">
                      <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
                      live
                    </span>
                  </div>
                  <div className="p-4 sm:p-5 font-mono text-[12.5px] leading-[1.7]">
                    {BOOT.map((line, i) => (
                      <div key={i} className="flex">
                        <span className="select-none w-6 shrink-0 text-text-muted">
                          {line.kind === 'in' ? '$' : ' '}
                        </span>
                        <span
                          className={
                            line.kind === 'ok'
                              ? 'text-success'
                              : line.kind === 'in'
                                ? 'text-text-primary'
                                : 'text-text-secondary'
                          }
                        >
                          {line.text}
                        </span>
                      </div>
                    ))}
                    <div className="mt-3 grid grid-cols-3 gap-3 max-w-md">
                      {[
                        { k: '<60s', v: 'first connect' },
                        { k: '0', v: 'config files' },
                        { k: '∞', v: 'devices paired' },
                      ].map((s) => (
                        <div key={s.v} className="rounded-md border border-border-soft bg-surface-alt/60 px-3 py-2.5">
                          <p className="font-sans text-[18px] font-semibold text-text-primary tabular-nums">
                            {s.k}
                          </p>
                          <p className="text-[10.5px] uppercase tracking-[0.12em] text-text-muted">
                            {s.v}
                          </p>
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
