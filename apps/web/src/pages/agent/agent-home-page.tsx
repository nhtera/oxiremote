import { useEffect, useState } from 'react'

// Host-dashboard home. Live-updates via the `/api/agent/events` SSE stream;
// initial snapshot from `/api/agent/state`. Both endpoints are localhost-only
// — the agent's `route_scope` middleware returns 403 over the tunnel.

type AgentState = {
  tunnel_url: string | null
  host_id: string
  label: string
  platform: string
  connected_devices: number
}

type AgentEvent =
  | { type: 'tunnel_url_changed'; url: string }
  | { type: 'device_connected'; device_id: string }
  | { type: 'device_disconnected'; device_id: string }
  | { type: 'device_pending'; device_id: string; ip: string; ua_parsed: string; first_seen: number }
  | { type: 'log_entry'; level: string; module: string; ts: number; msg: string }
  | { type: 'step_change'; name: string; status: string; sub?: string }

export default function AgentHomePage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [pending, setPending] = useState<{ device_id: string; ip: string; ua: string } | null>(null)

  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => r.json())
      .then((data: AgentState) => {
        if (!cancelled) setState(data)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

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
          setPending({ device_id: ev.device_id, ip: ev.ip, ua: ev.ua_parsed })
        }
      } catch {
        // drop malformed frames; server retries next event
      }
    }
    return () => es.close()
  }, [])

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
          <div className="text-sm text-text-muted">
            <p>One-time pairing keys arrive in Phase 02.</p>
            <p className="mt-2 text-xs">
              For now, use the pairing code printed in the agent's startup logs.
            </p>
          </div>
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

      {pending && (
        <PendingDeviceModal
          device={pending}
          onClose={() => setPending(null)}
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

// Placeholder for the Phase 02 approval modal. Phase 01 only displays that a
// device is pending; approve/reject handlers ship with the OTK flow.
function PendingDeviceModal({
  device,
  onClose,
}: {
  device: { device_id: string; ip: string; ua: string }
  onClose: () => void
}) {
  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4">
      <div className="bg-surface border border-accent rounded-lg max-w-md w-full p-5">
        <div className="text-accent font-semibold mb-2">Device pending approval</div>
        <div className="text-sm text-text-secondary space-y-1">
          <Row k="Device" v={device.device_id.slice(0, 12) + '…'} />
          <Row k="IP" v={device.ip} />
          <Row k="User-Agent" v={device.ua} />
        </div>
        <div className="mt-4 text-xs text-text-muted">
          Approval flow ships in Phase 02. Dismiss for now.
        </div>
        <div className="mt-4 flex justify-end">
          <button
            onClick={onClose}
            className="px-3 py-1.5 text-sm rounded-md border border-border text-text-secondary hover:text-text-primary"
          >
            Dismiss
          </button>
        </div>
      </div>
    </div>
  )
}
