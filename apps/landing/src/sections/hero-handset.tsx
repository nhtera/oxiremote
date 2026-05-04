import BrandMark from '../components/brand-mark'

// Decorative phone mock — right side of the hero. Shows the OxiRemote PWA on
// an iPhone-shaped frame with a terminal session in progress, mirroring the
// real desktop view but with mobile chrome.
export default function HeroHandset() {
  return (
    <div className="relative">
      {/* Floating glow */}
      <div className="absolute -inset-12 -z-10 float-orb">
        <div className="absolute top-12 left-6 w-40 h-40 rounded-full bg-accent/30 blur-[80px]" />
        <div className="absolute bottom-0 right-0 w-56 h-56 rounded-full bg-accent-deep/25 blur-[100px]" />
      </div>

      <div className="relative mx-auto w-[280px] sm:w-[300px] aspect-[9/19] rounded-[44px] bg-gradient-to-b from-[#1a1b22] to-[#0e0f13] border border-border/80 ring-glass p-2.5">
        {/* Notch */}
        <div className="absolute top-2.5 left-1/2 -translate-x-1/2 w-24 h-6 rounded-b-2xl bg-black/90 z-10" />

        <div className="relative w-full h-full rounded-[34px] overflow-hidden bg-surface flex flex-col">
          {/* Status bar */}
          <div className="px-5 pt-3 pb-1 flex items-center justify-between text-[11px] text-text-primary/90 font-mono">
            <span>9:41</span>
            <span className="flex items-center gap-1.5">
              <span className="inline-block w-3 h-3 align-middle">
                <svg viewBox="0 0 16 16" fill="currentColor" className="w-3 h-3"><path d="M2 6h2v6H2zm4-2h2v8H6zm4-2h2v10h-2zm4-2h2v12h-2z"/></svg>
              </span>
              <span className="inline-block w-3 h-3 align-middle">
                <svg viewBox="0 0 16 16" fill="currentColor" className="w-3 h-3"><path d="M8 3a8 8 0 0 1 5.7 2.4l-1.4 1.4A6 6 0 0 0 8 5a6 6 0 0 0-4.3 1.8L2.3 5.4A8 8 0 0 1 8 3zm0 4a4 4 0 0 1 2.8 1.2l-1.4 1.4A2 2 0 0 0 8 9a2 2 0 0 0-1.4.6L5.2 8.2A4 4 0 0 1 8 7zm0 4a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3z"/></svg>
              </span>
              <span className="inline-block w-6 h-3 rounded-sm border border-current relative">
                <span className="absolute inset-0.5 bg-success rounded-[1px]" style={{ width: '78%' }} />
              </span>
            </span>
          </div>

          {/* App header */}
          <div className="flex items-center gap-2 px-4 py-3 border-b border-border-soft">
            <BrandMark size={26} />
            <div className="flex flex-col leading-tight min-w-0">
              <span className="text-[11.5px] font-semibold text-text-primary truncate">atlas</span>
              <span className="text-[10px] text-success flex items-center gap-1">
                <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
                connected
              </span>
            </div>
            <button className="ml-auto w-7 h-7 rounded-md bg-surface-hover/60 inline-flex items-center justify-center" aria-label="More">
              <svg viewBox="0 0 16 16" className="w-3.5 h-3.5 text-text-secondary" fill="currentColor"><circle cx="3" cy="8" r="1.4"/><circle cx="8" cy="8" r="1.4"/><circle cx="13" cy="8" r="1.4"/></svg>
            </button>
          </div>

          {/* Terminal viewport */}
          <div className="flex-1 px-4 py-3 font-mono text-[10.5px] leading-[1.55] overflow-hidden">
            <p className="text-text-muted">atlas › main · clean</p>
            <p className="mt-1 text-text-primary">$ bun run dev</p>
            <p className="text-text-secondary">vite v8.0.9 ready in <span className="text-success">412ms</span></p>
            <p className="text-text-secondary">→ local:   <span className="text-info">http://localhost:5173</span></p>
            <p className="text-text-secondary">→ network: <span className="text-info">http://10.0.0.4:5173</span></p>
            <p className="mt-2 text-text-muted">[hmr] update /src/app.tsx in <span className="text-success">28ms</span></p>
            <p className="text-text-muted">[hmr] update /src/sections/pricing.tsx</p>
            <p className="mt-2 text-text-primary">$ <span className="caret inline-block w-[5px] h-[10px] -mb-px bg-accent/80 rounded-sm" /></p>
          </div>

          {/* Bottom dock */}
          <div className="px-3 pt-1 pb-4 flex items-center justify-around border-t border-border-soft bg-surface-alt/60">
            <DockBtn label="Term" active />
            <DockBtn label="Files" />
            <DockBtn label="Preview" />
            <DockBtn label="Desktop" />
          </div>
        </div>
      </div>

      {/* Floating notification — composed on top of phone */}
      <div className="absolute -left-6 sm:-left-12 top-32 w-64 rounded-2xl border border-border bg-surface-alt/85 backdrop-blur-md ring-glass p-3.5">
        <div className="flex items-start gap-3">
          <span className="shrink-0 mt-0.5 inline-flex items-center justify-center w-8 h-8 rounded-lg bg-accent/15 border border-accent/30 text-accent">
            <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M6 8a6 6 0 1 1 12 0c0 5 2 6 2 6H4s2-1 2-6"/><path d="M10 19a2 2 0 0 0 4 0"/></svg>
          </span>
          <div className="min-w-0">
            <p className="text-[12.5px] font-medium text-text-primary truncate">build done</p>
            <p className="text-[11.5px] text-text-secondary leading-snug">
              vite production build OK · 248 KB gz · 4.1s
            </p>
            <p className="text-[10.5px] font-mono text-text-muted mt-1">/h/atlas/terminal/sess-7c4</p>
          </div>
        </div>
      </div>

      {/* Floating status chip — bottom right of phone */}
      <div className="absolute -right-2 sm:-right-6 bottom-20 rounded-2xl border border-border bg-surface-alt/85 backdrop-blur-md ring-glass p-3 w-44">
        <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-text-muted">tunnel</p>
        <p className="text-[12px] font-semibold text-text-primary mt-1 truncate">atlas-zen-bay</p>
        <div className="mt-2 flex items-end gap-0.5 h-7">
          {[6, 9, 5, 14, 11, 18, 12, 21, 16, 24, 19, 26].map((h, i) => (
            <span
              key={i}
              className="w-1.5 rounded-sm bg-gradient-to-t from-accent/30 to-accent"
              style={{ height: `${h * 3}%` }}
            />
          ))}
        </div>
        <p className="mt-1.5 text-[10.5px] text-text-secondary">
          <span className="font-mono text-text-primary">38ms</span> p50 · <span className="font-mono text-text-primary">71ms</span> p95
        </p>
      </div>
    </div>
  )
}

function DockBtn({ label, active = false }: { label: string; active?: boolean }) {
  return (
    <span className={`flex flex-col items-center gap-1 px-1 py-0.5 ${active ? 'text-accent' : 'text-text-muted'}`}>
      <span className="w-7 h-7 rounded-lg bg-surface-hover/40 border border-border-soft inline-flex items-center justify-center">
        <span className="w-1.5 h-1.5 rounded-full bg-current opacity-80" />
      </span>
      <span className="text-[9px] font-medium tracking-wide">{label}</span>
    </span>
  )
}
