import { useEffect, useState } from 'react'
import PairingCard from '../../components/pairing-card'
import ApprovalModal from '../../components/approval-modal'
import ProxyPortsCard from '../../components/proxy-ports-card'
import AutoApproveToggle from '../../components/auto-approve-toggle'
import HealthCheckConsole, {
  type ProbeEntry,
} from '../../components/health-check-console'
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
  | {
      type: 'health_probe'
      attempt: number
      status: string
      elapsed_ms: number
      ok: boolean
    }

const PROBE_BUFFER = 20
const LOG_BUFFER = 50

type FetchStatus = 'loading' | 'ready' | 'error'

export default function AgentHomePage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [otk, setOtk] = useState<OtkState | null>(null)
  const [pendingDevice, setPendingDevice] = useState<PendingDevice | null>(null)
  const [otkError, setOtkError] = useState<string | null>(null)
  const [probeLog, setProbeLog] = useState<ProbeEntry[]>([])
  const [tunnelHealthy, setTunnelHealthy] = useState(false)
  const [tunnelDown, setTunnelDown] = useState(false)
  // Set true when tunnel_step_changed { step: 'ready' } fires — hides the progress card.
  const [tunnelStepReady, setTunnelStepReady] = useState(false)
  const [recentLogs, setRecentLogs] = useState<LogEntry[]>([])
  const [fetchStatus, setFetchStatus] = useState<FetchStatus>('loading')
  const [retryNonce, setRetryNonce] = useState(0)

  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json()
      })
      .then((data: AgentState) => {
        if (cancelled) return
        setState(data)
        setOtk(data.otk ?? null)
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
          setTunnelStepReady(true)
        } else if (ev.type === 'tunnel_url_changed') {
          setState((s) => (s ? { ...s, tunnel_url: ev.url } : s))
          setTunnelHealthy(false)
          setTunnelDown(false)
          setTunnelStepReady(false)
          setProbeLog([])
        } else if (ev.type === 'tunnel_down') {
          setTunnelDown(true)
          setTunnelHealthy(false)
        } else if (ev.type === 'health_probe') {
          if (ev.ok) setTunnelHealthy(true)
          setProbeLog((prev) => {
            const next = [
              ...prev,
              {
                attempt: ev.attempt,
                status: ev.status,
                ok: ev.ok,
                elapsed_ms: ev.elapsed_ms,
              },
            ]
            return next.length > PROBE_BUFFER
              ? next.slice(-PROBE_BUFFER)
              : next
          })
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
        }
      } catch {
        // drop malformed frames; server retries next event
      }
    }
    return () => es.close()
  }, [])

  const handleRegenOtk = async () => {
    setOtkError(null)
    try {
      const res = await fetch('/api/agent/keys/one-time', { method: 'POST' })
      if (!res.ok) throw new Error(`Failed to generate key (${res.status})`)
      const data: { token: string; expires_at: number } = await res.json()
      setOtk({ token: data.token, expires_at: data.expires_at })
    } catch (e) {
      setOtkError(e instanceof Error ? e.message : 'Failed to generate key')
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

  return (
    <div className="p-6 max-w-6xl mx-auto">
      {tunnelDown && (
        <div className="mb-4 px-4 py-3 rounded-md bg-danger/10 border border-danger/40 text-danger text-sm font-medium">
          Tunnel went down — connections will fail. Restart the agent to reconnect.
        </div>
      )}
      <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] gap-6">
        <aside className="lg:sticky lg:top-6 self-start space-y-4">
          <PairingCard
            tunnelUrl={tunnelUrl}
            otkToken={otk?.token ?? null}
            otkExpiresAt={otk?.expires_at ?? null}
            onRegenerate={handleRegenOtk}
            errorMessage={otkError}
          />

          {/* Tunnel progress checklist — visible while tunnel startup is in progress.
              Once tunnel_step_changed { step: 'ready' } fires and tunnel is healthy,
              the card disappears so the operator's attention stays on the pairing card. */}
          {!tunnelStepReady && !tunnelHealthy && (
            <TunnelProgressCard />
          )}
          {/* Legacy health-check probe console once we have a URL but no step events
              (e.g. named tunnel path that skips step events). */}
          {tunnelUrl && !tunnelHealthy && tunnelStepReady && (
            <Card title="Tunnel health">
              <HealthCheckConsole entries={probeLog} reachable={tunnelHealthy} />
            </Card>
          )}
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
