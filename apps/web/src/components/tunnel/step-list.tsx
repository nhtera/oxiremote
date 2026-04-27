import { useEffect, useState } from 'react'
import { StatusBadge, StepIcon } from './step-row'
import { defaultSub, TUNNEL_STEPS, type StepName, type StepState } from './step-types'

interface AgentEventTunnelDown { type: 'tunnel_down'; reason: string; recovery_hint?: string }
export interface AgentEventTunnelStep {
  type: 'tunnel_step_changed'
  step: 'preparing' | 'connecting' | 'tunneling' | 'verifying' | 'ready' | 'failed'
  attempt: number
  info?: string
  reason?: string
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

// 5-row checklist with live SSE updates. Shown only while the tunnel hasn't
// passed a health probe yet — parent (or the home page's onboarding gate)
// hides this once `tunnel_step_changed { step: 'ready' }` fires.
export default function TunnelStepList() {
  const [steps, setSteps] = useState<StepState[]>(buildInitialSteps)
  const [downHint, setDownHint] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false

    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (cancelled || !data?.tunnel_step) return
        const ev = data.tunnel_step as { type?: string }
        if (ev.type === 'tunnel_step_changed') {
          setSteps((prev) => applyStepEvent(prev, data.tunnel_step as AgentEventTunnelStep))
        }
      })
      .catch(() => { /* SSE will deliver fresh state */ })

    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type: string }
        if (ev.type === 'tunnel_step_changed') {
          setSteps((prev) => applyStepEvent(prev, ev as AgentEventTunnelStep))
        } else if (ev.type === 'tunnel_down') {
          const td = ev as AgentEventTunnelDown
          setDownHint(td.recovery_hint ?? td.reason)
          setSteps((prev) => {
            const next = prev.map((s) => ({ ...s }))
            const active = next.find(
              (s) => s.status === 'active' || (s.status === 'done' && s.name === 'Tunneling'),
            )
            if (active) {
              active.status = 'failed'
              active.info = td.recovery_hint ?? 'tunnel process exited'
            }
            return next
          })
        }
      } catch { /* drop malformed frames */ }
    }
    return () => { cancelled = true; es.close() }
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
          const titleTone =
            step.status === 'done' || step.status === 'active'
              ? 'text-text-primary'
              : step.status === 'failed'
                ? 'text-danger'
                : 'text-text-muted'
          return (
            <li key={step.name} className="flex items-center gap-3.5">
              <StepIcon status={step.status} />
              <div className="min-w-0 flex-1">
                <div className={`text-sm font-semibold leading-tight ${titleTone}`}>
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
