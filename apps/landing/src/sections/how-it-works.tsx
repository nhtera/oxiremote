import SectionEyebrow from '../components/section-eyebrow'

const STEPS = [
  {
    n: '01',
    title: 'Run one binary on your dev box',
    body:
      'macOS, Linux, Windows, or a Codespaces container. The agent downloads cloudflared, verifies SHA256, and binds 127.0.0.1:8787 — a single process, no daemon, no Docker.',
    code: '$ oxiremote',
  },
  {
    n: '02',
    title: 'Pair your phone with a QR',
    body:
      'A pairing code prints in the TUI. Scan the QR — your phone fetches the rotating tunnel URL from the discovery worker (or paste it manually) and exchanges the code for an Argon2id-hashed API key.',
    code: '3F8K · QR42 → key minted',
  },
  {
    n: '03',
    title: 'Live everywhere your laptop lives',
    body:
      'Terminals, files, dev-server previews, and full remote desktop appear in your phone’s browser as a PWA. Add to Home Screen and you have a real app — including push notifications.',
    code: '→ /h/atlas/terminal/sess-7c4',
  },
]

export default function HowItWorks() {
  return (
    <section id="how-it-works" className="relative py-24 sm:py-32">
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="reveal flex flex-col gap-5 max-w-2xl">
          <SectionEyebrow index="02" label="How it works" />
          <h2 className="heading-display text-[clamp(2rem,4.6vw,3.4rem)] text-text-primary">
            Three steps. <em className="text-text-secondary">Then it’s yours.</em>
          </h2>
          <p className="text-text-secondary text-[16px] max-w-xl leading-[1.55]">
            Nothing to sign up for. Nothing to expose to the internet. Your traffic
            rides your own Cloudflare tunnel — OxiRemote never sees it.
          </p>
        </div>

        <ol className="mt-16 grid grid-cols-1 lg:grid-cols-3 gap-px bg-border rounded-3xl overflow-hidden border border-border ring-glass">
          {STEPS.map((s, i) => (
            <li
              key={s.n}
              className="reveal relative bg-surface-alt p-8 sm:p-10 flex flex-col gap-6 min-h-[320px]"
              style={{ ['--reveal-delay' as string]: `${i * 80}ms` }}
            >
              <div className="flex items-baseline gap-3">
                <span className="font-mono text-[11px] tracking-[0.2em] text-accent uppercase">
                  Step
                </span>
                <span className="font-serif italic text-[44px] leading-none text-text-primary">
                  {s.n}
                </span>
              </div>

              <h3 className="text-[20px] sm:text-[22px] font-semibold tracking-tight text-text-primary leading-snug max-w-[18ch]">
                {s.title}
              </h3>

              <p className="text-[14px] text-text-secondary leading-[1.6] flex-1">
                {s.body}
              </p>

              <div className="rounded-lg border border-border-soft bg-surface px-3.5 py-2.5 font-mono text-[12px] text-text-secondary">
                {s.code}
              </div>
            </li>
          ))}
        </ol>
      </div>
    </section>
  )
}
