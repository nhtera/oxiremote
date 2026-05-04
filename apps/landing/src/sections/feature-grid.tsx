import SectionEyebrow from '../components/section-eyebrow'

interface Feature {
  title: string
  body: string
  span: string
  accent?: boolean
  visual: React.ReactNode
}

// Bento-style grid — six tiles, asymmetric column spans. Tile 0 is the wide
// hero card on accent; the others lean on density rather than imagery.
const FEATURES: Feature[] = [
  {
    title: 'Terminal that survives the subway',
    body:
      'xterm.js sessions persist across reconnects, not just retries. Background a long build, lose signal, walk into a tunnel — when you come back, the scrollback is exactly where you left it.',
    span: 'lg:col-span-2',
    accent: true,
    visual: <TerminalVisual />,
  },
  {
    title: 'Files at thumb speed',
    body:
      'Tree, search, rename, attach. Drag from your phone gallery into a chat. Long-press to share a temporary preview link.',
    span: 'lg:col-span-1',
    visual: <FilesVisual />,
  },
  {
    title: 'Remote desktop that doesn’t feel remote',
    body:
      'JPEG tile capture by default. Flip on H.264 with VideoToolbox or OpenH264 for a true 60fps mirror. Touch gestures, on-screen keyboard, paste batching.',
    span: 'lg:col-span-1',
    visual: <DesktopVisual />,
  },
  {
    title: 'Dev-server previews, anywhere',
    body:
      'Any local port becomes a tunnel-reachable URL. Share to your team without ngrok, without an account, without picking a region.',
    span: 'lg:col-span-1',
    visual: <PreviewVisual />,
  },
  {
    title: 'Push when it matters',
    body:
      '`oxiremote notify` from your shell wakes your phone. Tap the notification, land on the right host, the right session, the right cursor position.',
    span: 'lg:col-span-1',
    visual: <PushVisual />,
  },
  {
    title: 'Pairing built like a vault',
    body:
      'One-time code → Argon2id-hashed API key → CSRF cookie. Devices wait in a pending queue until you approve them. No keys, no accounts, no third party in the path.',
    span: 'lg:col-span-2',
    visual: <SecurityVisual />,
  },
]

export default function FeatureGrid() {
  return (
    <section id="features" className="relative">
      <div className="mx-auto max-w-7xl px-5 sm:px-8 py-24 sm:py-32">
        <div className="reveal flex flex-col gap-5 max-w-2xl">
          <SectionEyebrow index="01" label="Capabilities" />
          <h2 className="heading-display text-[clamp(2rem,4.6vw,3.4rem)] text-text-primary">
            Everything your laptop does.{' '}
            <em className="text-text-secondary">From the train.</em>
          </h2>
          <p className="text-text-secondary text-[16px] max-w-xl leading-[1.55]">
            Six surfaces, one binary. Each one tuned for a phone screen first,
            then a laptop screen second.
          </p>
        </div>

        <div className="mt-16 grid grid-cols-1 lg:grid-cols-3 gap-4">
          {FEATURES.map((f, i) => (
            <article
              key={f.title}
              className={`reveal relative group rounded-3xl overflow-hidden border ${
                f.accent
                  ? 'border-accent/30 bg-gradient-to-br from-accent/12 via-surface-alt to-surface-alt'
                  : 'border-border bg-surface-alt/70 backdrop-blur-sm'
              } ring-glass ${f.span}`}
              style={{ ['--reveal-delay' as string]: `${i * 60}ms` }}
            >
              <div className="p-7 sm:p-8 flex flex-col gap-5 h-full">
                <div className="h-44 sm:h-52 -mx-7 sm:-mx-8 -mt-7 sm:-mt-8 relative overflow-hidden border-b border-border-soft bg-surface/40">
                  {f.visual}
                </div>
                <div className="flex flex-col gap-2.5">
                  <h3 className="text-[18px] sm:text-[19px] font-semibold tracking-tight text-text-primary leading-snug">
                    {f.title}
                  </h3>
                  <p className="text-[14px] text-text-secondary leading-[1.6]">
                    {f.body}
                  </p>
                </div>
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  )
}

/* ---------------- visuals ---------------- */

function TerminalVisual() {
  return (
    <div className="absolute inset-0 p-5 font-mono text-[12px] leading-[1.6]">
      <div className="absolute inset-0 bg-dots opacity-30" />
      <div className="relative">
        <p className="text-text-muted">$ tail -f atlas/build.log</p>
        <p className="text-text-secondary mt-1">[14:02] vite build · entry: src/main.tsx</p>
        <p className="text-text-secondary">[14:02] tsc -b · 0 errors, 0 warnings</p>
        <p className="text-text-secondary">[14:03] gz · 248 KB initial · 47 KB lazy</p>
        <p className="text-success">[14:03] ✓ ready in 4.1s</p>
        <p className="text-text-muted">— signal lost · 3m 12s —</p>
        <p className="text-text-secondary">[14:06] resumed · scrollback intact</p>
        <p className="mt-1 flex">
          <span className="text-text-muted">$</span>
          <span className="caret w-[7px] h-[14px] ml-2 -mb-0.5 inline-block bg-accent/80 rounded-[1px]" />
        </p>
      </div>
    </div>
  )
}

function FilesVisual() {
  const rows = [
    { name: 'src', size: 'dir', open: true },
    { name: '  components', size: 'dir' },
    { name: '  sections', size: 'dir', highlight: true },
    { name: '    hero.tsx', size: '4.7 KB' },
    { name: '    feature-grid.tsx', size: '6.2 KB' },
    { name: 'package.json', size: '1.1 KB' },
  ]
  return (
    <div className="absolute inset-0 p-4 font-mono text-[11.5px]">
      <div className="rounded-lg border border-border-soft bg-surface-alt/60 overflow-hidden">
        <div className="px-3 py-1.5 border-b border-border-soft flex items-center gap-2 text-[10.5px] text-text-muted">
          <span className="inline-block w-2 h-2 rounded-sm bg-accent/60" /> ~/projects/atlas
        </div>
        {rows.map((r) => (
          <div
            key={r.name}
            className={`px-3 py-1 flex items-center justify-between ${
              r.highlight ? 'bg-accent/10 text-accent' : 'text-text-secondary'
            }`}
          >
            <span className="truncate">{r.name}</span>
            <span className="text-text-muted ml-2">{r.size}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function DesktopVisual() {
  return (
    <div className="absolute inset-0 p-5 flex items-center justify-center">
      <div className="absolute inset-0 bg-grid opacity-40" />
      <div className="relative w-full aspect-[16/10] rounded-md overflow-hidden border border-border-soft bg-gradient-to-br from-[#101218] to-[#1a1c25]">
        <div className="absolute top-0 inset-x-0 h-5 bg-surface-alt/80 border-b border-border-soft flex items-center px-2 gap-1">
          <span className="w-1.5 h-1.5 rounded-full bg-[#ff5f56]/70" />
          <span className="w-1.5 h-1.5 rounded-full bg-[#febc2e]/70" />
          <span className="w-1.5 h-1.5 rounded-full bg-[#28c840]/70" />
        </div>
        <div className="absolute top-5 inset-x-0 bottom-0 grid grid-cols-12 grid-rows-6 gap-1 p-1.5">
          <div className="col-span-3 row-span-6 rounded-sm bg-surface-hover/40" />
          <div className="col-span-9 row-span-1 rounded-sm bg-surface-hover/30" />
          <div className="col-span-9 row-span-5 rounded-sm bg-surface-hover/20 relative overflow-hidden">
            <span className="absolute top-2 left-2 right-2 h-2 rounded-sm bg-accent/40" />
            <span className="absolute top-6 left-2 w-1/2 h-1.5 rounded-sm bg-text-muted/30" />
            <span className="absolute top-9 left-2 w-1/3 h-1.5 rounded-sm bg-text-muted/30" />
          </div>
        </div>
        <div className="absolute bottom-2 right-2 inline-flex items-center gap-1.5 rounded-md bg-accent/20 border border-accent/40 px-1.5 py-0.5 font-mono text-[9px] text-accent">
          H.264 · 60fps
        </div>
      </div>
    </div>
  )
}

function PreviewVisual() {
  return (
    <div className="absolute inset-0 p-5 flex flex-col gap-2 font-mono text-[11.5px]">
      <div className="flex items-center justify-between text-text-muted">
        <span>localhost:5173</span>
        <span className="text-success">→ tunnel</span>
      </div>
      <div className="rounded-md border border-border-soft bg-surface-alt/70 p-2.5 text-text-secondary leading-relaxed">
        <span className="text-accent">https://</span>atlas-zen-bay
        <br />
        <span className="text-text-muted">.trycloudflare.com</span>
      </div>
      <div className="mt-auto flex items-center gap-2 text-text-secondary">
        <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
        <span>shared with 3 teammates</span>
      </div>
    </div>
  )
}

function PushVisual() {
  return (
    <div className="absolute inset-0 p-5 flex items-center">
      <div className="w-full rounded-2xl border border-border bg-surface-alt/80 backdrop-blur p-3.5 ring-glass">
        <div className="flex items-center gap-3">
          <span className="w-7 h-7 rounded-md bg-accent inline-flex items-center justify-center text-white">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" className="w-3.5 h-3.5"><rect x="2" y="3" width="12" height="9" rx="1.5" /><path d="M5 14h6M8 12v2"/></svg>
          </span>
          <div className="flex-1 min-w-0">
            <p className="text-[12px] font-medium text-text-primary">build done · atlas</p>
            <p className="text-[11px] text-text-secondary">vite production · 4.1s · 248 KB gz</p>
          </div>
          <span className="text-[10px] text-text-muted font-mono">14:03</span>
        </div>
        <p className="mt-2 text-[10.5px] font-mono text-text-muted">
          /h/atlas/terminal/sess-7c4
        </p>
      </div>
    </div>
  )
}

function SecurityVisual() {
  const steps = [
    { tag: '01', label: 'pairing code', value: '3F8K · QR42' },
    { tag: '02', label: 'argon2id key', value: 'k_…f8a2' },
    { tag: '03', label: 'csrf cookie', value: 'set · httponly' },
    { tag: '04', label: 'approval', value: 'pending → ok' },
  ]
  return (
    <div className="absolute inset-0 p-5 flex flex-col gap-2.5">
      <div className="absolute inset-0 bg-grid opacity-20" />
      {steps.map((s) => (
        <div
          key={s.tag}
          className="relative flex items-center gap-3 rounded-lg border border-border-soft bg-surface-alt/70 px-3 py-1.5"
        >
          <span className="font-mono text-[10px] text-text-muted">{s.tag}</span>
          <span className="text-[11.5px] text-text-secondary flex-1">{s.label}</span>
          <span className="font-mono text-[11px] text-text-primary">{s.value}</span>
        </div>
      ))}
    </div>
  )
}
