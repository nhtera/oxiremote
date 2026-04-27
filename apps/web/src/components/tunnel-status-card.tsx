import { useEffect, useState } from 'react'
import StatusChip from './ui/status-chip'

interface Props {
  tunnelUrl: string | null
  /** True once the agent's health probe has succeeded against the URL. */
  healthy: boolean
}

interface AgentEventTunnelUrlChanged { type: 'tunnel_url_changed'; url: string }
interface AgentEventHealthProbe { type: 'health_probe'; ok: boolean }
interface AgentEventTunnelDown { type: 'tunnel_down'; reason: string; recovery_hint?: string }
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

function StepIcon({ status }: { status: StepState['status'] }) {
  const base =
    'inline-flex w-7 h-7 items-center justify-center rounded-full shrink-0 ' +
    'transition-colors'
  if (status === 'done') {
    return (
      <span className={`${base} bg-accent text-white shadow-[0_0_0_3px_rgba(255,122,64,0.10)]`}>
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="w-3.5 h-3.5"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </span>
    )
  }
  if (status === 'active') {
    return (
      <span className={`${base} bg-accent/10 border border-accent/40 text-accent`}>
        <span className="inline-block w-2 h-2 rounded-full bg-accent animate-ping absolute" />
        <span className="inline-block w-2 h-2 rounded-full bg-accent" />
      </span>
    )
  }
  if (status === 'failed') {
    return (
      <span className={`${base} bg-danger/15 border border-danger/40 text-danger`}>
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="w-3.5 h-3.5"
        >
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </span>
    )
  }
  return (
    <span className={`${base} border border-border bg-surface-alt`}>
      <span className="inline-block w-1.5 h-1.5 rounded-full bg-text-muted/60" />
    </span>
  )
}

function defaultSub(name: StepName, status: StepState['status']): string | undefined {
  if (status === 'done') {
    switch (name) {
      case 'Preparing':
        return 'Tunnel binary ready'
      case 'Connecting':
        return 'Session established'
      case 'Tunneling':
        return 'Secure tunnel up'
      case 'Verifying':
        return 'Reachable from edge'
      case 'Ready':
        return 'Listening for clients'
    }
  }
  if (status === 'pending') {
    switch (name) {
      case 'Preparing':
        return 'Checking tunnel binary'
      case 'Connecting':
        return 'Creating session'
      case 'Tunneling':
        return 'Starting secure tunnel'
      case 'Verifying':
        return 'Probing edge reachability'
      case 'Ready':
        return 'Waiting for first client'
    }
  }
  return undefined
}

function StatusBadge({ status }: { status: StepState['status'] }) {
  if (status === 'done') {
    return (
      <span className="text-[11px] font-medium tracking-wide text-text-muted shrink-0">
        Done
      </span>
    )
  }
  if (status === 'active') {
    return (
      <span className="inline-flex items-center gap-1.5 text-[11px] font-medium tracking-wide text-accent bg-accent/10 border border-accent/30 rounded-full px-2 py-0.5 shrink-0">
        <span className="relative inline-flex w-1.5 h-1.5">
          <span className="absolute inline-flex w-full h-full rounded-full bg-accent opacity-60 animate-ping" />
          <span className="relative inline-flex w-1.5 h-1.5 rounded-full bg-accent" />
        </span>
        Running
      </span>
    )
  }
  if (status === 'failed') {
    return (
      <span className="text-[11px] font-medium tracking-wide text-danger shrink-0">
        Failed
      </span>
    )
  }
  return null
}

// TunnelProgressCard: self-subscribes to SSE and renders a 5-row checklist.
// Shown only while the tunnel hasn't passed a health probe yet. Once the
// tunnel_step_changed { step: 'ready' } event arrives the parent can hide us.
export function TunnelProgressCard() {
  const [steps, setSteps] = useState<StepState[]>(buildInitialSteps)
  const [downHint, setDownHint] = useState<string | null>(null)

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
          const td = ev as AgentEventTunnelDown
          setDownHint(td.recovery_hint ?? td.reason)
          setSteps((prev) => {
            const next = prev.map((s) => ({ ...s }))
            const active = next.find((s) => s.status === 'active' || s.status === 'done' && s.name === 'Tunneling')
            if (active) {
              active.status = 'failed'
              active.info = td.recovery_hint ?? 'tunnel process exited'
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
    <div className="rounded-2xl border border-border bg-surface-alt/60 backdrop-blur-sm shadow-[0_1px_0_rgba(255,255,255,0.02)_inset,0_8px_28px_-12px_rgba(0,0,0,0.6)] p-5 md:p-6">
      <div className="flex items-center justify-between gap-3 mb-5">
        <div className="text-[11px] uppercase tracking-[0.18em] text-text-muted font-medium">
          Setting up connection
        </div>
        <span className="inline-flex items-center gap-1.5 text-[11px] text-text-muted">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-accent animate-pulse" />
          Live
        </span>
      </div>
      {downHint && (
        <div className="mb-4 rounded-lg border border-danger/40 bg-danger/10 p-3 text-xs text-danger">
          <div className="font-semibold mb-1">Tunnel down</div>
          <div className="text-danger/85 leading-relaxed">{downHint}</div>
        </div>
      )}
      <ol className="space-y-3">
        {steps.map((step) => {
          const sub = step.info ?? defaultSub(step.name, step.status)
          return (
            <li key={step.name} className="flex items-center gap-3.5">
              <StepIcon status={step.status} />
              <div className="min-w-0 flex-1">
                <div
                  className={
                    'text-sm font-semibold leading-tight ' +
                    (step.status === 'done'
                      ? 'text-text-primary'
                      : step.status === 'active'
                        ? 'text-text-primary'
                        : step.status === 'failed'
                          ? 'text-danger'
                          : 'text-text-muted')
                  }
                >
                  {step.name}
                </div>
                {sub && (
                  <div
                    className={
                      'text-xs truncate mt-0.5 ' +
                      (step.status === 'failed' ? 'text-danger/80' : 'text-text-muted')
                    }
                    title={sub}
                  >
                    {sub}
                  </div>
                )}
              </div>
              <StatusBadge status={step.status} />
            </li>
          )
        })}
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
  const [down, setDown] = useState<{ reason: string; hint?: string } | null>(null)

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
          setDown(null)
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
          const td = ev as AgentEventTunnelDown
          setDown({ reason: td.reason, hint: td.recovery_hint })
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
  // Hover tooltip carries the recovery hint when present so the operator
  // sees what to do next without leaving the layout header.
  const title = down ? (down.hint ?? down.reason) : undefined
  return <span title={title}><StatusChip variant={variant}>{label}</StatusChip></span>
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
