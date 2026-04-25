import { useEffect, useState } from 'react'
import OneTimeKeyField from '../../components/one-time-key-field'
import ApprovalModal from '../../components/approval-modal'
import ProxyPortsCard from '../../components/proxy-ports-card'

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
  | { type: 'log_entry'; level: string; module: string; ts: number; msg: string }
  | { type: 'step_change'; name: string; status: string; sub?: string }

export default function AgentHomePage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [otk, setOtk] = useState<OtkState | null>(null)
  const [pendingDevice, setPendingDevice] = useState<PendingDevice | null>(null)
  const [otkError, setOtkError] = useState<string | null>(null)

  // Fetch initial state (includes otk if present)
  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => r.json())
      .then((data: AgentState) => {
        if (!cancelled) {
          setState(data)
          setOtk(data.otk ?? null)
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  // Subscribe to SSE events
  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev: AgentEvent = JSON.parse(msg.data)
        if (ev.type === 'tunnel_url_changed') {
          setState((s) => (s ? { ...s, tunnel_url: ev.url } : s))
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

  return (
    <div className="p-6 max-w-4xl mx-auto space-y-6">
      <header>
        <h1 className="text-xl font-semibold text-text-primary">Host Dashboard</h1>
        <p className="text-sm text-text-muted mt-1">
          {state ? `${state.label} · ${state.platform}` : 'Loading…'}
        </p>
      </header>

      <section className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card title="Tunnel URL">
          {tunnelUrl ? (
            <div className="flex flex-col gap-3 items-start">
              <img
                src={`/api/agent/qr?url=${encodeURIComponent(tunnelUrl)}`}
                alt="Tunnel QR code"
                className="w-48 h-48 rounded-md bg-white p-2 border border-border"
              />
              <a
                className="text-xs text-accent break-all hover:underline"
                href={tunnelUrl}
                target="_blank"
                rel="noreferrer"
              >
                {tunnelUrl}
              </a>
            </div>
          ) : (
            <div className="text-sm text-text-muted">
              Tunnel not ready yet — check agent logs.
            </div>
          )}
        </Card>

        <Card title="One-Time Key">
          <OneTimeKeyField
            token={otk?.token ?? null}
            expiresAt={otk?.expires_at ?? 0}
            onRegenerate={handleRegenOtk}
          />
          {otkError && (
            <div className="mt-2 text-xs text-danger bg-danger/10 border border-danger/30 rounded px-2 py-1">
              {otkError}
            </div>
          )}
        </Card>

        <Card title="Host">
          <Row k="Host ID" v={state?.host_id ?? '—'} />
          <Row k="Label" v={state?.label ?? '—'} />
          <Row k="Platform" v={state?.platform ?? '—'} />
        </Card>

        <Card title="Connected Devices">
          <div className="text-3xl font-semibold text-text-primary">
            {state?.connected_devices ?? '—'}
          </div>
          <div className="text-xs text-text-muted mt-1">
            Active terminal/preview sessions
          </div>
        </Card>
      </section>

      <section>
        <Card title="Local Sites Proxy">
          <ProxyPortsCard tunnelUrl={tunnelUrl} />
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
