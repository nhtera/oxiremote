import { useEffect, useState } from 'react'
import StatusChip from './ui/status-chip'

interface Props {
  tunnelUrl: string | null
  /** True once the agent's health probe has succeeded against the URL. */
  healthy: boolean
}

interface AgentEventTunnelUrlChanged { type: 'tunnel_url_changed'; url: string }
interface AgentEventHealthProbe { type: 'health_probe'; ok: boolean }
interface AgentEventTunnelDown { type: 'tunnel_down'; reason: string }
interface AgentEventTunnelStep {
  type: 'tunnel_step_changed'
  step: 'preparing' | 'connecting' | 'tunneling' | 'verifying' | 'ready' | 'failed'
  attempt: number
  info?: string
  reason?: string // present when step === 'failed'
}
type PillEvent = AgentEventTunnelUrlChanged | AgentEventHealthProbe | AgentEventTunnelDown | AgentEventTunnelStep | { type: string }

// --- 5-step tunnel progress card -------------------------------------------

const TUNNEL_STEPS = ['Preparing', 'Connecting', 'Tunneling', 'Verifying', 'Ready'] as const
type StepName = (typeof TUNNEL_STEPS)[number]

interface StepState {
  name: StepName
  status: 'done' | 'active' | 'pending' | 'failed'
  info?: string
}

function buildInitialSteps(): StepState[] {
  return TUNNEL_STEPS.map((name, i) => ({
    name,
    status: i === 0 ? 'active' : 'pending',
  }))
}

function applyStepEvent(steps: StepState[], ev: AgentEventTunnelStep): StepState[] {
  const next = steps.map((s) => ({ ...s }))
  const stepKey = ev.step as string

  if (stepKey === 'failed') {
    // Mark the active step failed.
    for (const s of next) {
      if (s.status === 'active') {
        s.status = 'failed'
        s.info = ev.reason ?? 'unknown error'
      }
    }
    return next
  }

  const nameMap: Record<string, StepName> = {
    preparing: 'Preparing',
    connecting: 'Connecting',
    tunneling: 'Tunneling',
    verifying: 'Verifying',
    ready: 'Ready',
  }
  const targetName = nameMap[stepKey]
  if (!targetName) return next

  const targetIdx = TUNNEL_STEPS.indexOf(targetName)
  for (let i = 0; i < next.length; i++) {
    if (i < targetIdx) {
      next[i].status = 'done'
      // Clear stale sub-text from when this step was Active, so it doesn't
      // linger under the green check while the spinner is on a later row.
      next[i].info = undefined
    } else if (i === targetIdx) {
      next[i].status = stepKey === 'ready' ? 'done' : 'active'
      next[i].info = ev.info
    } else {
      next[i].status = 'pending'
      next[i].info = undefined
    }
  }
  return next
}

// Pulsing orange spinner for the active step.
function StepSpinner() {
  return (
    <span className="inline-block w-3.5 h-3.5 rounded-full border-2 border-accent border-t-transparent animate-spin shrink-0" />
  )
}

function StepIcon({ status }: { status: StepState['status'] }) {
  if (status === 'done') {
    return (
      <span className="inline-flex w-3.5 h-3.5 items-center justify-center shrink-0 text-success">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </span>
    )
  }
  if (status === 'active') return <StepSpinner />
  if (status === 'failed') {
    return (
      <span className="inline-flex w-3.5 h-3.5 items-center justify-center shrink-0 text-danger">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
          <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </span>
    )
  }
  // pending
  return <span className="inline-block w-3.5 h-3.5 rounded-full border border-border bg-surface-alt shrink-0" />
}

// TunnelProgressCard: self-subscribes to SSE and renders a 5-row checklist.
// Shown only while the tunnel hasn't passed a health probe yet. Once the
// tunnel_step_changed { step: 'ready' } event arrives the parent can hide us.
export function TunnelProgressCard() {
  const [steps, setSteps] = useState<StepState[]>(buildInitialSteps)

  useEffect(() => {
    let cancelled = false

    // Hydrate from the snapshot first — the SSE stream has no replay, so a
    // page reload mid-startup would otherwise leave us frozen at the default
    // "Preparing" state forever. `tunnel_step` mirrors the latest event; if
    // null the agent hasn't emitted any step yet (truly preparing).
    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (cancelled || !data?.tunnel_step) return
        const ev = data.tunnel_step as { type?: string }
        if (ev.type === 'tunnel_step_changed') {
          setSteps((prev) => applyStepEvent(prev, data.tunnel_step as AgentEventTunnelStep))
        }
      })
      .catch(() => { /* SSE stream below will deliver fresh state */ })

    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type: string }
        if (ev.type === 'tunnel_step_changed') {
          setSteps((prev) => applyStepEvent(prev, ev as AgentEventTunnelStep))
        } else if (ev.type === 'tunnel_down') {
          // Show tunnel down as a failed active step.
          setSteps((prev) => {
            const next = prev.map((s) => ({ ...s }))
            const active = next.find((s) => s.status === 'active' || s.status === 'done' && s.name === 'Tunneling')
            if (active) {
              active.status = 'failed'
              active.info = 'tunnel process exited'
            }
            return next
          })
        }
      } catch { /* drop malformed frames */ }
    }
    return () => {
      cancelled = true
      es.close()
    }
  }, [])

  return (
    <div className="rounded-lg border border-border bg-surface p-4">
      <div className="text-xs uppercase tracking-wide text-text-muted mb-3">Tunnel startup</div>
      <ol className="space-y-2">
        {steps.map((step) => (
          <li key={step.name} className="flex items-start gap-2.5">
            <span className="mt-0.5">
              <StepIcon status={step.status} />
            </span>
            <div className="min-w-0">
              <span
                className={
                  'text-sm font-medium ' +
                  (step.status === 'done'
                    ? 'text-text-primary'
                    : step.status === 'active'
                      ? 'text-accent'
                      : step.status === 'failed'
                        ? 'text-danger'
                        : 'text-text-muted')
                }
              >
                {step.name}
              </span>
              {step.info && (
                <div className="text-xs text-text-muted truncate mt-0.5" title={step.info}>
                  {step.info}
                </div>
              )}
            </div>
          </li>
        ))}
      </ol>
    </div>
  )
}

// Compact pill version of the tunnel status — mounts in the agent-layout
// header so the operator sees connectivity at a glance from any /agent/* page.
// Self-fetches /api/agent/state and subscribes to SSE so the layout doesn't
// have to know anything about tunnel state. Cheap: localhost-only SSE with
// at most a handful of internal subscribers.
export function TunnelStatusPill() {
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null)
  const [healthy, setHealthy] = useState(false)
  const [down, setDown] = useState(false)

  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (cancelled || !data) return
        setTunnelUrl(data.tunnel_url ?? null)
        // Hydrate healthy from the snapshot — if the latest tunnel step is
        // `ready`, the tunnel is up even if we open the page after the
        // single `health_probe { ok: true }` event already fired.
        const ts = data.tunnel_step as AgentEventTunnelStep | null | undefined
        if (ts?.type === 'tunnel_step_changed' && ts.step === 'ready') {
          setHealthy(true)
        }
      })
      .catch(() => {
        // The dashboard's /api/agent/events stream will resync once it lands.
      })

    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as PillEvent
        if (ev.type === 'tunnel_url_changed') {
          setTunnelUrl((ev as AgentEventTunnelUrlChanged).url)
          setHealthy(false)
          setDown(false)
        } else if (ev.type === 'health_probe' && (ev as AgentEventHealthProbe).ok) {
          setHealthy(true)
        } else if (
          ev.type === 'tunnel_step_changed'
          && (ev as AgentEventTunnelStep).step === 'ready'
        ) {
          // First-client-connected wins the verify race even before the HTTP
          // probe succeeds. Surface it on the pill too.
          setHealthy(true)
        } else if (ev.type === 'tunnel_down') {
          setDown(true)
          setHealthy(false)
        }
      } catch {
        // Drop malformed frames; SSE keep-alive will deliver fresh state.
      }
    }
    return () => {
      cancelled = true
      es.close()
    }
  }, [])

  const variant = down ? 'rejected' : !tunnelUrl ? 'offline' : healthy ? 'online' : 'pending'
  const label = down
    ? 'Tunnel down'
    : !tunnelUrl
      ? 'Starting'
      : healthy
        ? 'Reachable'
        : 'Probing'
  return <StatusChip variant={variant}>{label}</StatusChip>
}

// Tunnel status banner — second-most-prominent element on /agent (after the
// pairing card). Shows the URL big, lets the operator copy / open it, and
// surfaces the health verdict via StatusChip. When the URL hasn't appeared yet
// (cloudflared still booting) we render a "starting" state instead of hiding,
// so the operator knows what's happening.
export default function TunnelStatusCard({ tunnelUrl, healthy }: Props) {
  const [copied, setCopied] = useState(false)

  async function copyUrl() {
    if (!tunnelUrl) return
    try {
      await navigator.clipboard.writeText(tunnelUrl)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      // clipboard can fail in non-secure contexts; ignore — the URL is visible.
    }
  }

  // Variant precedence: explicit healthy ✓ wins; URL present but not yet
  // probed = pending; null URL = offline (cloudflared still spawning).
  const variant = !tunnelUrl ? 'offline' : healthy ? 'online' : 'pending'
  const label = !tunnelUrl
    ? 'Starting tunnel'
    : healthy
      ? 'Reachable'
      : 'Probing'

  return (
    <section className="rounded-lg border border-border bg-surface p-4">
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
