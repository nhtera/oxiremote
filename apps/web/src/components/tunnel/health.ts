// Phase 4: tri-state tunnel health derived from `tunnel_step_changed` events.
// `verifying` = the agent's own probe is inconclusive but cloudflared has
// registered with Cloudflare's edge (real clients via Cloudflare's resolver
// can still pair). `degraded` = the probe definitively saw the tunnel fail
// (DoH NXDOMAIN, 5xx, transport error). Both the header pill and the
// TunnelStatusCard derive their pill colour from this same shape.

export type TunnelHealth =
  | { kind: 'unknown' }
  | { kind: 'ready' }
  | { kind: 'verifying'; reason: string }
  | { kind: 'degraded'; reason: string }

export interface TunnelStepEvent {
  type: 'tunnel_step_changed'
  step: 'preparing' | 'connecting' | 'tunneling' | 'verifying' | 'ready' | 'degraded' | 'failed'
  attempt: number
  info?: string
  reason?: string
}

/**
 * Map a `tunnel_step_changed` event to a tunnel-health verdict, or `null`
 * when the event doesn't change the verdict (preparing / connecting / etc).
 */
export function healthFromStepEvent(ev: TunnelStepEvent): TunnelHealth | null {
  if (ev.step === 'ready') {
    // The probe-task tags its soft-fallback Ready with this exact suffix.
    if (ev.info?.includes('local probe inconclusive')) {
      return { kind: 'verifying', reason: 'local probe inconclusive — scan QR to test' }
    }
    return { kind: 'ready' }
  }
  if (ev.step === 'degraded') {
    return { kind: 'degraded', reason: ev.reason ?? 'tunnel unhealthy' }
  }
  return null
}
