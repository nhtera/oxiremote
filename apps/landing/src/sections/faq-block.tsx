import { useState } from 'react'
import SectionEyebrow from '../components/section-eyebrow'

interface QA {
  q: string
  a: React.ReactNode
}

const QAS: QA[] = [
  {
    q: 'Where does my traffic actually go?',
    a: (
      <>
        Through your own Cloudflare Quick Tunnel (or named tunnel, if configured).
        OxiRemote never proxies your bytes — the agent talks directly to{' '}
        <code className="font-mono text-text-primary text-[13px] bg-surface-alt border border-border rounded px-1 py-0.5">
          cloudflared
        </code>
        , and the connection terminates at your machine.
      </>
    ),
  },
  {
    q: 'How is authentication handled?',
    a: (
      <>
        A pairing code is consumed once and exchanged for an Argon2id-hashed API
        key bound to your device. State-changing requests over the tunnel also
        require a CSRF cookie. Devices land in a pending queue until you approve
        them — or flip <code className="font-mono text-text-primary text-[13px] bg-surface-alt border border-border rounded px-1 py-0.5">auto_approve</code> if you trust the LAN.
      </>
    ),
  },
  {
    q: 'Does it work on iOS without an App Store install?',
    a: (
      <>
        Yes — the web UI is a PWA. Add to Home Screen and you get an app-like
        shell with offline cache, on-screen keyboard tuned for terminals, and
        Web Push notifications you can deep-link from your shell.
      </>
    ),
  },
  {
    q: 'JPEG vs H.264 for remote desktop?',
    a: (
      <>
        Default is JPEG tile-capture over WebRTC DataChannel — works everywhere,
        no codec deal-breakers, ~30fps. Build with the{' '}
        <code className="font-mono text-text-primary text-[13px] bg-surface-alt border border-border rounded px-1 py-0.5">h264</code>{' '}
        feature flag and the agent will use VideoToolbox on macOS or OpenH264 on
        Linux for a 60fps WebRTC video track when both ends agree.
      </>
    ),
  },
  {
    q: 'What about persistence across reconnects?',
    a: (
      <>
        Terminal sessions, file uploads, and tunnel state survive client
        disconnects. The agent keeps PTYs alive, and the SPA reattaches to the
        existing session by ID — your scrollback and process state are exactly
        where you left them.
      </>
    ),
  },
  {
    q: 'Can I run this on a server I never sit at?',
    a: (
      <>
        That’s the Codespaces story. Headless mode (
        <code className="font-mono text-text-primary text-[13px] bg-surface-alt border border-border rounded px-1 py-0.5">
          oxiremote --auto
        </code>
        ) prints the QR to the boot log; the included{' '}
        <code className="font-mono text-text-primary text-[13px] bg-surface-alt border border-border rounded px-1 py-0.5">
          .devcontainer/
        </code>{' '}
        starts the agent on every Codespace boot.
      </>
    ),
  },
]

export default function FaqBlock() {
  const [open, setOpen] = useState<number>(0)

  return (
    <section className="relative py-24 sm:py-32">
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-10">
          <div className="lg:col-span-4 reveal">
            <SectionEyebrow index="05" label="Questions" />
            <h2 className="heading-display mt-4 text-[clamp(2rem,4.4vw,3rem)] text-text-primary">
              The fine print, <em className="text-text-secondary">unfine-printed.</em>
            </h2>
            <p className="mt-4 text-text-secondary text-[15px] leading-[1.55]">
              Plain answers to the questions everyone asks the first time they
              point a phone at their dev box.
            </p>
          </div>

          <div className="lg:col-span-8 reveal" style={{ ['--reveal-delay' as string]: '120ms' }}>
            <ul className="rounded-2xl border border-border bg-surface-alt/60 backdrop-blur-sm overflow-hidden ring-glass divide-y divide-border-soft">
              {QAS.map((qa, i) => {
                const isOpen = open === i
                return (
                  <li key={qa.q}>
                    <button
                      type="button"
                      onClick={() => setOpen(isOpen ? -1 : i)}
                      className="w-full flex items-center gap-4 px-5 sm:px-7 py-5 text-left group"
                      aria-expanded={isOpen}
                    >
                      <span className="font-mono text-[11px] text-text-muted tabular-nums w-8 shrink-0">
                        {String(i + 1).padStart(2, '0')}
                      </span>
                      <span className="flex-1 text-[15.5px] sm:text-[16px] font-medium text-text-primary tracking-tight">
                        {qa.q}
                      </span>
                      <span
                        className={`shrink-0 w-7 h-7 rounded-full border flex items-center justify-center transition-all duration-300 ${
                          isOpen
                            ? 'bg-accent text-white border-accent rotate-45'
                            : 'border-border text-text-secondary group-hover:border-text-secondary'
                        }`}
                      >
                        <svg viewBox="0 0 16 16" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                          <path d="M8 3v10M3 8h10" />
                        </svg>
                      </span>
                    </button>
                    <div
                      className="grid transition-all duration-300 ease-out"
                      style={{ gridTemplateRows: isOpen ? '1fr' : '0fr' }}
                    >
                      <div className="overflow-hidden">
                        <p className="px-5 sm:px-7 pb-6 pl-[68px] text-[14.5px] text-text-secondary leading-[1.65] max-w-3xl">
                          {qa.a}
                        </p>
                      </div>
                    </div>
                  </li>
                )
              })}
            </ul>
          </div>
        </div>
      </div>
    </section>
  )
}
