import { useEffect, useState } from 'react'
import StatusChip from '../ui/status-chip'
import { healthFromStepEvent, type TunnelHealth, type TunnelStepEvent } from './health'

interface AgentEventTunnelUrlChanged { type: 'tunnel_url_changed'; url: string }
interface AgentEventHealthProbe { type: 'health_probe'; ok: boolean }
interface AgentEventTunnelDown { type: 'tunnel_down'; reason: string; recovery_hint?: string }
type PillEvent =
  | AgentEventTunnelUrlChanged
  | AgentEventHealthProbe
  | AgentEventTunnelDown
  | TunnelStepEvent
  | { type: string }

// Compact pill for the agent-layout header — visible from every /agent/* page.
// Self-fetches /api/agent/state and subscribes to SSE so the layout doesn't
// need to know about tunnel state. Cheap: localhost-only with a handful of
// internal subscribers.
export default function TunnelStatusPill() {
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null)
  const [health, setHealth] = useState<TunnelHealth>({ kind: 'unknown' })
  const [down, setDown] = useState<{ reason: string; hint?: string } | null>(null)

  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (cancelled || !data) return
        setTunnelUrl(data.tunnel_url ?? null)
        const ts = data.tunnel_step as TunnelStepEvent | null | undefined
        if (ts?.type === 'tunnel_step_changed') {
          const h = healthFromStepEvent(ts)
          if (h) setHealth(h)
        }
      })
      .catch(() => { /* SSE will resync */ })

    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as PillEvent
        if (ev.type === 'tunnel_url_changed') {
          setTunnelUrl((ev as AgentEventTunnelUrlChanged).url)
          setHealth({ kind: 'unknown' })
          setDown(null)
        } else if (ev.type === 'health_probe' && (ev as AgentEventHealthProbe).ok) {
          setHealth({ kind: 'ready' })
        } else if (ev.type === 'tunnel_step_changed') {
          const h = healthFromStepEvent(ev as TunnelStepEvent)
          if (h) setHealth(h)
        } else if (ev.type === 'tunnel_down') {
          const td = ev as AgentEventTunnelDown
          setDown({ reason: td.reason, hint: td.recovery_hint })
          setHealth({ kind: 'unknown' })
        }
      } catch { /* drop malformed frames */ }
    }
    return () => {
      cancelled = true
      es.close()
    }
  }, [])

  let variant: 'offline' | 'pending' | 'online' | 'rejected'
  let label: string
  let title: string | undefined
  if (down) {
    variant = 'rejected'
    label = 'Tunnel down'
    title = down.hint ?? down.reason
  } else if (!tunnelUrl) {
    variant = 'offline'
    label = 'Starting'
  } else if (health.kind === 'ready') {
    variant = 'online'
    label = 'Reachable'
  } else if (health.kind === 'verifying') {
    variant = 'pending'
    label = 'Verifying'
    title = health.reason
  } else if (health.kind === 'degraded') {
    variant = 'rejected'
    label = 'Tunnel unhealthy'
    title = health.reason
  } else {
    variant = 'pending'
    label = 'Probing'
  }
  return <span title={title}><StatusChip variant={variant}>{label}</StatusChip></span>
}
