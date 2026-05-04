// Decorative terminal mock — left card in the hero composition. Lives in its
// own file because it carries a non-trivial chunk of static markup that would
// otherwise drown the hero file. The output is hand-crafted to look like an
// actual `oxiremote` boot sequence rather than generic "running command" copy.
export default function HeroTerminal() {
  return (
    <div className="relative rounded-2xl border border-border bg-surface-alt/85 backdrop-blur-md ring-glass overflow-hidden">
      <div className="flex items-center gap-2 px-4 py-3 border-b border-border-soft">
        <span className="w-2.5 h-2.5 rounded-full bg-[#ff5f56]/80" />
        <span className="w-2.5 h-2.5 rounded-full bg-[#febc2e]/80" />
        <span className="w-2.5 h-2.5 rounded-full bg-[#28c840]/80" />
        <span className="ml-3 font-mono text-[11px] text-text-muted truncate">
          ~/projects/atlas — oxiremote
        </span>
        <span className="ml-auto inline-flex items-center gap-1.5 text-[11px] text-text-secondary">
          <span className="w-1.5 h-1.5 rounded-full bg-success live-dot" />
          live
        </span>
      </div>

      <div className="p-5 font-mono text-[12.5px] leading-[1.65]">
        <Line prompt>
          <span className="text-text-secondary">oxiremote</span>{' '}
          <span className="text-text-muted">--auto</span>
        </Line>
        <Line>
          <span className="text-info">→</span> downloading cloudflared{' '}
          <span className="text-text-muted">2024.10.0</span>{' '}
          <span className="text-success">ok</span>
        </Line>
        <Line>
          <span className="text-info">→</span> sha256 verified{' '}
          <span className="text-text-muted">8a3c…f2e1</span>
        </Line>
        <Line>
          <span className="text-info">→</span> binding port{' '}
          <span className="text-text-primary">8787</span>{' '}
          <span className="text-success">ok</span>
        </Line>
        <Line>
          <span className="text-info">→</span> opening quick tunnel
        </Line>
        <Line>
          <span className="text-success">✓</span> reachable at{' '}
          <span className="text-accent underline-offset-2">
            https://atlas-zen-bay.trycloudflare.com
          </span>
        </Line>

        <div className="mt-4 rounded-lg border border-border/70 bg-surface/70 p-3 grid grid-cols-[120px_1fr] gap-x-4 gap-y-1 text-[12px]">
          <span className="text-text-muted">pairing code</span>
          <span className="text-text-primary tracking-[0.18em]">3F8K · QR42</span>
          <span className="text-text-muted">workspace</span>
          <span className="text-text-primary truncate">~/projects/atlas</span>
          <span className="text-text-muted">device limit</span>
          <span className="text-text-primary">5 approved · 0 pending</span>
        </div>

        <div className="mt-4 flex items-center gap-2">
          <span className="text-text-muted">›</span>
          <span className="text-text-secondary">scan QR or paste tunnel URL on phone</span>
          <span className="caret w-[7px] h-[14px] -mb-0.5 inline-block bg-accent/80 rounded-[1px]" />
        </div>
      </div>
    </div>
  )
}

function Line({ children, prompt = false }: { children: React.ReactNode; prompt?: boolean }) {
  return (
    <div className="flex">
      <span className="select-none w-5 shrink-0 text-text-muted">{prompt ? '$' : ' '}</span>
      <span className="flex-1">{children}</span>
    </div>
  )
}
