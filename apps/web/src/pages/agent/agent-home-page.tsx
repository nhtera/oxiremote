import { useEffect, useState } from 'react'
import PairingCard from '../../components/pairing-card'
import ApprovalModal from '../../components/approval-modal'
import ProxyPortsCard from '../../components/proxy-ports-card'
import AutoApproveToggle from '../../components/auto-approve-toggle'
import RecentLogsCard, { type LogEntry } from '../../components/recent-logs-card'
import PermissionsWidget from '../../components/permissions-widget'
import DevicesPanel from '../../components/devices-panel'
import { TunnelProgressCard } from '../../components/tunnel-status-card'
import { Button, SkeletonCard, StateView } from '../../components/ui'

// Host-dashboard home. Live-updates via the `/api/agent/events` SSE stream;
// initial snapshot from `/api/agent/state`. Both endpoints are localhost-only
// — the agent's `route_scope` middleware returns 403 over the tunnel.
//
// Layout: 2-col grid at `lg:` (≥1024px). Left column = sticky PairingCard
// (the hero — operator's primary action). Right column = host info + devices
// + permissions + proxy + recent logs. Tunnel-status pill lives in the
// agent-layout header so it's visible from every /agent/* page.

interface OtkState {
  token: string
  expires_at: number
}

type AgentState = {
  tunnel_url: string | null
  host_id: string
  label: string
  platform: string
  connected_devices: number
  auto_approve?: boolean
  otk?: OtkState | null
}

interface PendingDevice {
  device_id: string
  ip: string
  ua_parsed: string
  first_seen: number
}

interface PermanentKeyMeta {
  last4: string
  created_at: number
}

type AgentEvent =
  | { type: 'tunnel_url_changed'; url: string }
  | { type: 'tunnel_down'; reason: string }
  | { type: 'device_connected'; device_id: string }
  | { type: 'device_disconnected'; device_id: string }
  | { type: 'device_pending'; device_id: string; ip: string; ua_parsed: string; first_seen: number }
  | { type: 'device_approved'; device_id: string }
  | { type: 'device_rejected'; device_id: string }
  | { type: 'otk_issued'; token_prefix: string }
  | { type: 'otk_used'; token_prefix: string }
  | { type: 'otk_expired'; token_prefix: string }
  | { type: 'log_entry'; level: 'info' | 'warn' | 'error'; module: string; ts: number; msg: string }
  | { type: 'step_change'; name: string; status: string; sub?: string }
  | { type: 'tunnel_step_changed'; step: string; attempt: number; info?: string; reason?: string }
  | { type: 'health_probe'; attempt: number; status: string; elapsed_ms: number; ok: boolean }
  | { type: 'permanent_key_rotated'; last4: string }

const LOG_BUFFER = 50

type FetchStatus = 'loading' | 'ready' | 'error'

export default function AgentHomePage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [otk, setOtk] = useState<OtkState | null>(null)
  const [pendingDevice, setPendingDevice] = useState<PendingDevice | null>(null)
  const [otkError, setOtkError] = useState<string | null>(null)
  const [tunnelHealthy, setTunnelHealthy] = useState(false)
  const [tunnelDown, setTunnelDown] = useState(false)
  // Set true when tunnel_step_changed { step: 'ready' } fires — hides the progress card.
  const [tunnelStepReady, setTunnelStepReady] = useState(false)
  const [recentLogs, setRecentLogs] = useState<LogEntry[]>([])
  const [fetchStatus, setFetchStatus] = useState<FetchStatus>('loading')
  const [retryNonce, setRetryNonce] = useState(0)
  const [permanentKey, setPermanentKey] = useState<PermanentKeyMeta | null>(null)
  const [revealedPlaintext, setRevealedPlaintext] = useState<string | null>(null)
  // Confirm modal for OTK regeneration — prevents accidental QR invalidation.
  const [confirmingOtk, setConfirmingOtk] = useState(false)

  useEffect(() => {
    let cancelled = false

    // Fetch agent state and permanent key metadata in parallel.
    Promise.all([
      fetch('/api/agent/state').then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json() as Promise<AgentState & { tunnel_step?: { type?: string; step?: string } | null }>
      }),
      fetch('/api/agent/keys/permanent').then((r) => {
        // 404 means no key generated yet — not an error.
        if (r.status === 404) return null
        if (!r.ok) return null
        return r.json() as Promise<PermanentKeyMeta>
      }),
    ])
      .then(([agentData, keyMeta]) => {
        if (cancelled) return
        setState(agentData)
        setOtk(agentData.otk ?? null)
        // Seed `tunnelStepReady` from the snapshot so the progress card hides
        // immediately on a page reload after the tunnel is already healthy.
        if (agentData.tunnel_step?.type === 'tunnel_step_changed' && agentData.tunnel_step.step === 'ready') {
          setTunnelStepReady(true)
          setTunnelHealthy(true)
        }
        setPermanentKey(keyMeta)
        setFetchStatus('ready')
      })
      .catch(() => {
        if (!cancelled) setFetchStatus('error')
      })
    return () => {
      cancelled = true
    }
  }, [retryNonce])

  function retry() {
    setFetchStatus('loading')
    setRetryNonce((n) => n + 1)
  }

  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev: AgentEvent = JSON.parse(msg.data)
        if (ev.type === 'tunnel_step_changed' && ev.step === 'ready') {
          // Ready proves the tunnel works (HTTP probe succeeded OR a real
          // client just hit it). Bridge to tunnelHealthy so the onboarding
          // gate flips even when the first-client race wins the verify step.
          setTunnelStepReady(true)
          setTunnelHealthy(true)
          setTunnelDown(false)
        } else if (ev.type === 'tunnel_url_changed') {
          setState((s) => (s ? { ...s, tunnel_url: ev.url } : s))
          setTunnelHealthy(false)
          setTunnelDown(false)
          setTunnelStepReady(false)
        } else if (ev.type === 'tunnel_down') {
          setTunnelDown(true)
          setTunnelHealthy(false)
        } else if (ev.type === 'health_probe') {
          if (ev.ok) setTunnelHealthy(true)
        } else if (ev.type === 'log_entry') {
          setRecentLogs((prev) => {
            const next = [
              ...prev,
              { level: ev.level, module: ev.module, ts: ev.ts, msg: ev.msg },
            ]
            return next.length > LOG_BUFFER ? next.slice(-LOG_BUFFER) : next
          })
        } else if (ev.type === 'device_connected') {
          setState((s) => (s ? { ...s, connected_devices: s.connected_devices + 1 } : s))
        } else if (ev.type === 'device_disconnected') {
          setState((s) =>
            s ? { ...s, connected_devices: Math.max(0, s.connected_devices - 1) } : s,
          )
        } else if (ev.type === 'device_pending') {
          setPendingDevice({
            device_id: ev.device_id,
            ip: ev.ip,
            ua_parsed: ev.ua_parsed,
            first_seen: ev.first_seen,
          })
        } else if (ev.type === 'device_approved' || ev.type === 'device_rejected') {
          setPendingDevice((d) => (d?.device_id === ev.device_id ? null : d))
        } else if (ev.type === 'otk_expired') {
          setOtk((o) => (o ? { ...o, expires_at: 0 } : o))
        } else if (ev.type === 'permanent_key_rotated') {
          // Another dashboard tab rotated the key — refresh metadata from the
          // server. We can't reconstruct created_at from the event alone.
          fetch('/api/agent/keys/permanent')
            .then((r) => (r.ok ? r.json() : null))
            .then((meta: PermanentKeyMeta | null) => {
              if (meta) setPermanentKey(meta)
            })
            .catch(() => { /* non-critical; stale meta is acceptable */ })
        }
      } catch {
        // drop malformed frames; server retries next event
      }
    }
    return () => es.close()
  }, [])

  const handleRegenOtk = async () => {
    setOtkError(null)
    setConfirmingOtk(false)
    try {
      const res = await fetch('/api/agent/keys/one-time', { method: 'POST' })
      if (!res.ok) throw new Error(`Failed to generate key (${res.status})`)
      const data: { token: string; expires_at: number } = await res.json()
      setOtk({ token: data.token, expires_at: data.expires_at })
    } catch (e) {
      setOtkError(e instanceof Error ? e.message : 'Failed to generate key')
    }
  }

  const handlePermanentRegen = async () => {
    setOtkError(null)
    try {
      const res = await fetch('/api/agent/keys/permanent', { method: 'POST' })
      if (!res.ok) throw new Error(`Failed to rotate permanent key (${res.status})`)
      const data: { api_key: string; last4: string; created_at: number } = await res.json()
      // Reveal the plaintext once — state cleared by onDismissReveal.
      setRevealedPlaintext(data.api_key)
      setPermanentKey({ last4: data.last4, created_at: data.created_at })
    } catch (e) {
      // Surface as otkError for simplicity; both show in the same card.
      setOtkError(e instanceof Error ? e.message : 'Failed to rotate permanent key')
    }
  }

  const handleApprove = async () => {
    if (!pendingDevice) return
    await fetch(`/api/agent/approvals/${pendingDevice.device_id}/approve`, { method: 'POST' })
    setPendingDevice(null)
  }

  const handleReject = async () => {
    if (!pendingDevice) return
    await fetch(`/api/agent/approvals/${pendingDevice.device_id}/reject`, { method: 'POST' })
    setPendingDevice(null)
  }

  const tunnelUrl = state?.tunnel_url ?? null
  // Onboarding mode: hide the full dashboard until the tunnel is both
  // step-ready AND healthy. Independent of fetchStatus — the progress card
  // self-fetches and self-subscribes, so we don't need /api/agent/state.
  const onboarding = !tunnelStepReady || !tunnelHealthy

  if (fetchStatus === 'error') {
    return (
      <div className="p-6 max-w-4xl mx-auto">
        <StateView
          tone="error"
          icon={
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="h-full w-full" aria-hidden="true">
              <circle cx="12" cy="12" r="10" />
              <line x1="12" y1="8" x2="12" y2="12" />
              <line x1="12" y1="16" x2="12.01" y2="16" />
            </svg>
          }
          title="Couldn't reach the agent"
          body="The local oxiremote process isn't responding. Make sure it's running, then retry."
          action={<Button variant="primary" onClick={retry}>Retry</Button>}
        />
      </div>
    )
  }

  if (onboarding) {
    return (
      <div className="min-h-[80vh] flex items-start justify-center pt-16 md:pt-24 px-4">
        <div className="w-full max-w-lg space-y-5">
          <div className="text-center space-y-2">
            <h1 className="text-[length:var(--text-h1)] font-semibold tracking-tight text-text-primary">
              Bringing your tunnel online
            </h1>
            <p className="text-sm text-text-secondary leading-relaxed max-w-md mx-auto">
              Cloudflare is opening a secure outbound tunnel to your machine.
              No port forwarding, no inbound exposure.
            </p>
          </div>
          {tunnelDown && (
            <div className="px-4 py-3 rounded-lg bg-danger/10 border border-danger/40 text-danger text-sm font-medium">
              Tunnel went down — connections will fail. Restart the agent to reconnect.
            </div>
          )}
          <TunnelProgressCard />
        </div>
      </div>
    )
  }

  return (
    <div className="p-6 max-w-6xl mx-auto">
      {tunnelDown && (
        <div className="mb-4 px-4 py-3 rounded-md bg-danger/10 border border-danger/40 text-danger text-sm font-medium">
          Tunnel went down — connections will fail. Restart the agent to reconnect.
        </div>
      )}
      <div className="grid grid-cols-1 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)] gap-6">
        <aside className="lg:sticky lg:top-6 self-start space-y-4">
          <PairingCard
            tunnelUrl={tunnelUrl}
            otkToken={otk?.token ?? null}
            otkExpiresAt={otk?.expires_at ?? null}
            onRegenerate={() => setConfirmingOtk(true)}
            errorMessage={otkError}
            permanentKey={permanentKey}
            revealedPlaintext={revealedPlaintext}
            onRegeneratePermanent={handlePermanentRegen}
            onDismissReveal={() => setRevealedPlaintext(null)}
          />
        </aside>

        <section className="space-y-4">
          {fetchStatus === 'loading' ? (
            <>
              <SkeletonCard lines={3} />
              <SkeletonCard lines={3} />
            </>
          ) : (
            <>
              <Card title="Host">
                <Row k="Host ID" v={state?.host_id ?? '—'} />
                <Row k="Label" v={state?.label ?? '—'} />
                <Row k="Platform" v={state?.platform ?? '—'} />
              </Card>

              <Card
                title="Connected Devices"
                action={
                  state ? (
                    <AutoApproveToggle
                      enabled={state.auto_approve ?? false}
                      onChange={(next) =>
                        setState((s) => (s ? { ...s, auto_approve: next } : s))
                      }
                    />
                  ) : null
                }
              >
                <div className="flex items-baseline gap-2 mb-3">
                  <div className="text-[length:var(--text-display)] font-semibold text-text-primary leading-none">
                    {state?.connected_devices ?? '—'}
                  </div>
                  <div className="text-[length:var(--text-meta)] text-text-muted">
                    active terminal/preview sessions
                  </div>
                </div>
                <DevicesPanel />
              </Card>

              <Card title="Remote Desktop Permissions">
                <PermissionsWidget />
              </Card>

              <Card title="Local Sites Proxy">
                <ProxyPortsCard tunnelUrl={tunnelUrl} />
              </Card>

              <Card title="Recent Logs">
                <RecentLogsCard entries={recentLogs} />
              </Card>
            </>
          )}
        </section>
      </div>

      {pendingDevice && (
        <ApprovalModal
          device={pendingDevice}
          onApprove={handleApprove}
          onReject={handleReject}
          onClose={() => setPendingDevice(null)}
        />
      )}

      {confirmingOtk && (
        <OtkConfirmModal
          onConfirm={handleRegenOtk}
          onCancel={() => setConfirmingOtk(false)}
        />
      )}
    </div>
  )
}

function Card({
  title,
  action,
  children,
}: {
  title: string
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div className="rounded-lg border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-3 mb-3">
        <div className="text-xs uppercase tracking-wide text-text-muted">{title}</div>
        {action}
      </div>
      {children}
    </div>
  )
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex justify-between py-1 text-sm">
      <span className="text-text-muted">{k}</span>
      <span className="text-text-primary truncate ml-3 max-w-[60%]" title={v}>
        {v}
      </span>
    </div>
  )
}

interface OtkConfirmModalProps {
  onConfirm: () => void
  onCancel: () => void
}

// Confirm dialog before invalidating the current OTK. Uses the same overlay
// style as ApprovalModal so the two modals feel like the same system.
function OtkConfirmModal({ onConfirm, onCancel }: OtkConfirmModalProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm p-4"
      onClick={onCancel}
    >
      <div
        className="w-full max-w-sm bg-surface border border-border rounded-lg p-5 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="text-text-primary font-semibold text-sm mb-2">
          Generate a new one-time key?
        </div>
        <p className="text-xs text-text-secondary leading-relaxed mb-5">
          The current QR will stop working immediately. A device scanning right
          now will need the new key.
        </p>
        <div className="flex gap-2 justify-end">
          <button
            onClick={onCancel}
            className="px-4 py-2 text-sm font-medium border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="px-4 py-2 text-sm font-medium bg-accent/20 text-accent border border-accent/40 rounded-md hover:bg-accent/30 transition-colors"
          >
            Generate
          </button>
        </div>
      </div>
    </div>
  )
}
