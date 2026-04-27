import { useState } from 'react'
import StatusChip from './ui/status-chip'
import TunnelStatusPillImpl from './tunnel/status-pill'
import TunnelStepList from './tunnel/step-list'

interface Props {
  tunnelUrl: string | null
  /** True once the agent's health probe has succeeded against the URL. */
  healthy: boolean
}

// Re-exports for existing call sites — composers landed in `./tunnel/`.
export const TunnelStatusPill = TunnelStatusPillImpl
export const TunnelProgressCard = TunnelStepList

// Tunnel status banner — second-most-prominent element on /agent (after the
// pairing card). Shows the URL big, lets the operator copy / open it, and
// surfaces the health verdict via StatusChip. When the URL hasn't appeared yet
// (cloudflared still booting) we render a "starting" state instead of hiding.
export default function TunnelStatusCard({ tunnelUrl, healthy }: Props) {
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

  const variant = !tunnelUrl ? 'offline' : healthy ? 'online' : 'pending'
  const label = !tunnelUrl ? 'Starting tunnel' : healthy ? 'Reachable' : 'Probing'

  return (
    <section className="rounded-xl border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-3 mb-2">
        <div className="text-[length:var(--text-h3)] uppercase tracking-wide text-text-muted">
          Tunnel
        </div>
        <StatusChip variant={variant}>{label}</StatusChip>
      </div>

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
