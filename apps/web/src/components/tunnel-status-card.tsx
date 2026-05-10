import { useState } from 'react'
import StatusChip from './ui/status-chip'
import { type TunnelHealth } from './tunnel/health'
import TunnelStatusPillImpl from './tunnel/status-pill'
import TunnelStepList from './tunnel/step-list'

interface Props {
  tunnelUrl: string | null
  /** Phase-4 tri-state probe verdict. `verifying` is amber (probe inconclusive
   *  but cellular phones via Cloudflare's resolver can pair); `degraded` is
   *  red (DoH NXDOMAIN, 5xx, transport error). */
  health: TunnelHealth
}

// Re-exports for existing call sites — composers landed in `./tunnel/`.
export const TunnelStatusPill = TunnelStatusPillImpl
export const TunnelProgressCard = TunnelStepList

// Tunnel status banner — second-most-prominent element on /agent (after the
// pairing card). Shows the URL big, lets the operator copy / open it, and
// surfaces the health verdict via StatusChip. When the URL hasn't appeared yet
// (cloudflared still booting) we render a "starting" state instead of hiding.
export default function TunnelStatusCard({ tunnelUrl, health }: Props) {
  const [copied, setCopied] = useState(false)

  async function copyUrl() {
    if (!tunnelUrl) return
    try {
      await navigator.clipboard.writeText(tunnelUrl)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      // clipboard can fail in non-secure contexts; URL is still visible.
    }
  }

  let variant: 'offline' | 'pending' | 'online' | 'rejected'
  let label: string
  let chipTitle: string | undefined
  let banner: { tone: 'amber' | 'red'; text: string } | null = null
  if (!tunnelUrl) {
    variant = 'offline'
    label = 'Starting tunnel'
  } else if (health.kind === 'ready') {
    variant = 'online'
    label = 'Reachable'
  } else if (health.kind === 'verifying') {
    variant = 'pending'
    label = 'Verifying'
    chipTitle = health.reason
    banner = { tone: 'amber', text: health.reason }
  } else if (health.kind === 'degraded') {
    variant = 'rejected'
    label = 'Tunnel unhealthy'
    chipTitle = health.reason
    banner = { tone: 'red', text: health.reason }
  } else {
    variant = 'pending'
    label = 'Probing'
  }

  return (
    <section className="rounded-xl border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-3 mb-2">
        <div className="text-[length:var(--text-h3)] uppercase tracking-wide text-text-muted">
          Tunnel
        </div>
        <span title={chipTitle}>
          <StatusChip variant={variant}>{label}</StatusChip>
        </span>
      </div>
      {banner && (
        <div
          className={
            banner.tone === 'red'
              ? 'mb-2 rounded-md border border-danger/40 bg-danger/10 px-2.5 py-1.5 text-xs text-danger'
              : 'mb-2 rounded-md border border-amber-400/40 bg-amber-400/10 px-2.5 py-1.5 text-xs text-amber-300'
          }
        >
          {banner.text}
        </div>
      )}

      {tunnelUrl ? (
        <div className="flex items-center gap-2 min-w-0">
          <code className="flex-1 min-w-0 font-mono text-[length:var(--text-mono)] text-text-primary truncate select-all">
            {tunnelUrl}
          </code>
          <button
            onClick={copyUrl}
            className="shrink-0 px-2.5 py-1 text-[length:var(--text-meta)] bg-surface-alt text-text-secondary border border-border rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
            aria-label="Copy tunnel URL"
          >
            {copied ? 'Copied' : 'Copy'}
          </button>
          <a
            href={tunnelUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="shrink-0 px-2.5 py-1 text-[length:var(--text-meta)] bg-accent/15 text-accent border border-accent/30 rounded-md hover:bg-accent/25 transition-colors"
          >
            Open
          </a>
        </div>
      ) : (
        <div className="text-[length:var(--text-body)] text-text-muted">
          Waiting for cloudflared to publish a URL. Check{' '}
          <a href="/agent/logs" className="text-accent hover:text-accent-hover">
            logs
          </a>{' '}
          if this hangs more than ~10 s.
        </div>
      )}

      <div className="mt-2 text-[length:var(--text-meta)] text-text-muted">
        Quick tunnel — rotates on restart. Switch to a Named Tunnel via{' '}
        <code className="font-mono">~/.config/oxiremote/tunnel.toml</code>.
      </div>
    </section>
  )
}
