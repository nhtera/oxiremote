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
import TunnelStatusCard from '../../components/tunnel-status-card'
import { Button, SkeletonCard, StateView } from '../../components/ui'

// Host-dashboard home. Live-updates via the `/api/agent/events` SSE stream;
// initial snapshot from `/api/agent/state`. Both endpoints are localhost-only
// — the agent's `route_scope` middleware returns 403 over the tunnel.

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
  const [recentLogs, setRecentLogs] = useState<LogEntry[]>([])
  const [fetchStatus, setFetchStatus] = useState<FetchStatus>('loading')
  const [retryNonce, setRetryNonce] = useState(0)

  // Fetch initial state (includes otk if present). Surface fetch failures so
  // the user gets a "couldn't reach agent" panel instead of an empty page.
  // Retry button bumps retryNonce → re-runs this effect.
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

  // Subscribe to SSE events
  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev: AgentEvent = JSON.parse(msg.data)
        if (ev.type === 'tunnel_url_changed') {
          setState((s) => (s ? { ...s, tunnel_url: ev.url } : s))
          // New tunnel URL → reset health-check state so the console shows
          // probes again instead of leaving the previous "reachable" badge.
          setTunnelHealthy(false)
          setProbeLog([])
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
          // Clear modal if it was for this device
          setPendingDevice((d) => (d?.device_id === ev.device_id ? null : d))
        } else if (ev.type === 'otk_expired') {
          // Mark token as expired by setting expires_at to past
          setOtk((o) => (o ? { ...o, expires_at: 0 } : o))
        }
      } catch {
        // drop malformed frames; server retries next event
      }
    }
    return () => es.close()
  }, [])

  // Regenerate OTK via POST /api/agent/keys/one-time
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

  // Approve a pending device
  const handleApprove = async () => {
    if (!pendingDevice) return
    await fetch(`/api/agent/approvals/${pendingDevice.device_id}/approve`, { method: 'POST' })
    setPendingDevice(null)
  }

  // Reject a pending device
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
    <div className="p-6 max-w-4xl mx-auto space-y-6">
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-[length:var(--text-h1)] font-semibold tracking-tight text-text-primary">Host Dashboard</h1>
          <p className="text-[length:var(--text-meta)] text-text-muted mt-1">
            {state ? `${state.label} · ${state.platform}` : 'Loading…'}
          </p>
        </div>
        {state && (
          <AutoApproveToggle
            enabled={state.auto_approve ?? false}
            onChange={(next) =>
              setState((s) => (s ? { ...s, auto_approve: next } : s))
            }
          />
        )}
      </header>

      {/* Tunnel status banner — second-most-prominent element after pairing.
          Hidden while the very first /api/agent/state response is in flight
          to avoid a flash of "tunnel unreachable" copy. */}
      {fetchStatus === 'ready' && (
        <TunnelStatusCard tunnelUrl={tunnelUrl} healthy={tunnelHealthy} />
      )}

      {/* Hero: pairing card combines QR + OTK + URL + countdown. PairingCard
          carries its own loading-state for the OTK so we render it eagerly. */}
      <PairingCard
        tunnelUrl={tunnelUrl}
        otkToken={otk?.token ?? null}
        otkExpiresAt={otk?.expires_at ?? null}
        onRegenerate={handleRegenOtk}
        errorMessage={otkError}
      />

      {/* Tunnel health check is only useful while the tunnel hasn't passed
          a probe yet — once it has, hide the noise. */}
      {tunnelUrl && !tunnelHealthy && (
        <Card title="Tunnel health">
          <HealthCheckConsole entries={probeLog} reachable={tunnelHealthy} />
        </Card>
      )}
      {!tunnelUrl && fetchStatus === 'ready' && (
        <Card title="Tunnel health">
          <HealthCheckConsole entries={probeLog} reachable={false} />
        </Card>
      )}

      <section className="grid grid-cols-1 md:grid-cols-2 gap-4">
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

            <Card title="Connected Devices">
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
          </>
        )}
      </section>

      <section>
        <Card title="Remote Desktop Permissions">
          <PermissionsWidget />
        </Card>
      </section>

      <section>
        <Card title="Local Sites Proxy">
          <ProxyPortsCard tunnelUrl={tunnelUrl} />
        </Card>
      </section>

      <section>
        <Card title="Recent Logs">
          <RecentLogsCard entries={recentLogs} />
        </Card>
      </section>

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

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border bg-surface p-4">
      <div className="text-xs uppercase tracking-wide text-text-muted mb-3">{title}</div>
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
