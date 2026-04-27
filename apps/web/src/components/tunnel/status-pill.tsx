import { useEffect, useState } from 'react'
import StatusChip from '../ui/status-chip'

interface AgentEventTunnelUrlChanged { type: 'tunnel_url_changed'; url: string }
interface AgentEventHealthProbe { type: 'health_probe'; ok: boolean }
interface AgentEventTunnelDown { type: 'tunnel_down'; reason: string; recovery_hint?: string }
interface AgentEventTunnelStep {
  type: 'tunnel_step_changed'
  step: 'preparing' | 'connecting' | 'tunneling' | 'verifying' | 'ready' | 'failed'
  attempt: number
  info?: string
  reason?: string
}
type PillEvent =
  | AgentEventTunnelUrlChanged
  | AgentEventHealthProbe
  | AgentEventTunnelDown
  | AgentEventTunnelStep
  | { type: string }

// Compact pill for the agent-layout header — visible from every /agent/* page.
// Self-fetches /api/agent/state and subscribes to SSE so the layout doesn't
// need to know about tunnel state. Cheap: localhost-only with a handful of
// internal subscribers.
export default function TunnelStatusPill() {
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
        const ts = data.tunnel_step as AgentEventTunnelStep | null | undefined
        if (ts?.type === 'tunnel_step_changed' && ts.step === 'ready') {
          setHealthy(true)
        }
      })
      .catch(() => { /* SSE will resync */ })

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
          setHealthy(true)
        } else if (ev.type === 'tunnel_down') {
          const td = ev as AgentEventTunnelDown
          setDown({ reason: td.reason, hint: td.recovery_hint })
          setHealthy(false)
        }
      } catch { /* drop malformed frames */ }
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
  const title = down ? (down.hint ?? down.reason) : undefined
  return <span title={title}><StatusChip variant={variant}>{label}</StatusChip></span>
}
