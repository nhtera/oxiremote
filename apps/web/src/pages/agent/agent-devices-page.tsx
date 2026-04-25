import { useEffect, useMemo, useState } from 'react'
import { Button, StateView, StatusChip, type StatusVariant } from '../../components/ui'

interface PendingDevice {
  device_id: string
  ip: string
  ua_parsed: string
  first_seen: number
}

interface TrustedDevice {
  device_id: string
  label?: string
  approval_status: 'approved' | 'pending' | 'rejected'
  last_seen?: number
}

type AnyDevice =
  | ({ kind: 'pending' } & PendingDevice)
  | ({ kind: 'trusted' } & TrustedDevice)

function statusVariant(status: string): StatusVariant {
  if (status === 'approved') return 'online'
  if (status === 'pending') return 'pending'
  if (status === 'rejected') return 'rejected'
  return 'offline'
}

export default function AgentDevicesPage() {
  const [devices, setDevices] = useState<AnyDevice[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)

  const fetchDevices = async () => {
    setError(null)
    try {
      // Both endpoints are localhost-scoped — agent_api.rs serves them.
      // Pending: bare array. Trusted: bare array (NOT { devices: [...] }; that
      // shape lives on the public /api/devices route, which needs a session).
      const [pendingRes, trustedRes] = await Promise.all([
        fetch('/api/agent/approvals/pending'),
        fetch('/api/agent/devices'),
      ])
      const pending: PendingDevice[] = pendingRes.ok ? await pendingRes.json() : []
      const trusted: TrustedDevice[] = trustedRes.ok ? await trustedRes.json() : []

      // Pending entries take priority — drop their trusted shadow if any.
      const pendingIds = new Set(pending.map((d) => d.device_id))
      const merged: AnyDevice[] = [
        ...pending.map((d) => ({ kind: 'pending' as const, ...d })),
        ...trusted
          .filter((d) => !pendingIds.has(d.device_id))
          .map((d) => ({ kind: 'trusted' as const, ...d })),
      ]
      setDevices(merged)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to load devices')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    // Standard fetch-on-mount; lint thinks setState in an effect is suspicious
    // but here it's the documented pattern for "load initial data once".
    // eslint-disable-next-line react-hooks/set-state-in-effect
    fetchDevices()
  }, [])

  async function approve(id: string) {
    setBusyId(id)
    try {
      await fetch(`/api/agent/approvals/${id}/approve`, { method: 'POST' })
      await fetchDevices()
    } finally {
      setBusyId(null)
    }
  }

  async function reject(id: string) {
    setBusyId(id)
    try {
      await fetch(`/api/agent/approvals/${id}/reject`, { method: 'POST' })
      await fetchDevices()
    } finally {
      setBusyId(null)
    }
  }

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return devices
    return devices.filter((d) => {
      const label = d.kind === 'trusted' ? (d.label ?? '') : ''
      const ip = d.kind === 'pending' ? d.ip : ''
      return (
        d.device_id.toLowerCase().includes(q) ||
        label.toLowerCase().includes(q) ||
        ip.toLowerCase().includes(q)
      )
    })
  }, [devices, query])

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between mb-4 gap-3">
        <div>
          <h1 className="text-[length:var(--text-h1)] font-semibold text-text-primary">
            Devices
          </h1>
          <p className="text-[length:var(--text-meta)] text-text-muted mt-0.5">
            Paired devices and pending approvals
          </p>
        </div>
        <Button onClick={fetchDevices} size="sm">
          Refresh
        </Button>
      </div>

      {error && (
        <div className="mb-4 px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-[length:var(--text-meta)]">
          {error}
        </div>
      )}

      <div className="mb-3">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter by id, label, or ip…"
          className="w-full bg-surface border border-border rounded-md px-3 py-2 text-[length:var(--text-body)] text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent/50"
        />
      </div>

      {loading ? (
        <div className="text-[length:var(--text-meta)] text-text-muted py-8 text-center">
          Loading…
        </div>
      ) : devices.length === 0 ? (
        <StateView
          icon={
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="h-full w-full" aria-hidden="true">
              <rect x="5" y="2" width="14" height="20" rx="2" />
              <line x1="12" y1="18" x2="12.01" y2="18" />
            </svg>
          }
          title="No devices yet"
          body="Pair one with the QR code or one-time key from the host dashboard."
        />
      ) : filtered.length === 0 ? (
        <StateView
          title="No matches"
          body={`Nothing matches "${query}".`}
        />
      ) : (
        <div className="rounded-lg border border-border overflow-hidden">
          <table className="w-full text-[length:var(--text-meta)]">
            <thead>
              <tr className="border-b border-border bg-surface-alt">
                <Th>Device</Th>
                <Th>Label / IP</Th>
                <Th>User Agent</Th>
                <Th>Status</Th>
                <Th>Last seen</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((d) => (
                <DeviceRow
                  key={d.device_id}
                  device={d}
                  busy={busyId === d.device_id}
                  onApprove={() => approve(d.device_id)}
                  onReject={() => reject(d.device_id)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}

function Th({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <th
      className={[
        'text-left px-4 py-2.5 text-[length:var(--text-meta)] text-text-muted font-medium uppercase tracking-wide',
        className ?? '',
      ]
        .filter(Boolean)
        .join(' ')}
    >
      {children}
    </th>
  )
}

interface RowProps {
  device: AnyDevice
  busy: boolean
  onApprove: () => void
  onReject: () => void
}

function DeviceRow({ device, busy, onApprove, onReject }: RowProps) {
  const shortId = `…${device.device_id.slice(-8)}`
  const status = device.kind === 'pending' ? 'pending' : device.approval_status
  const variant = statusVariant(status)
  const ipOrLabel =
    device.kind === 'pending' ? device.ip : (device.label ?? '—')
  const ua = device.kind === 'pending' ? device.ua_parsed : '—'
  const lastSeen =
    device.kind === 'trusted' && device.last_seen
      ? fmtRelative(device.last_seen)
      : device.kind === 'pending'
        ? fmtRelative(device.first_seen)
        : '—'

  return (
    <tr className="border-b border-border last:border-0 hover:bg-surface-alt transition-colors">
      <td
        className="px-4 py-3 font-mono text-text-primary"
        title={device.device_id}
      >
        {shortId}
      </td>
      <td className="px-4 py-3 text-text-secondary">{ipOrLabel}</td>
      <td
        className="px-4 py-3 text-text-muted max-w-[200px] truncate"
        title={ua}
      >
        {ua}
      </td>
      <td className="px-4 py-3">
        <StatusChip variant={variant}>{status}</StatusChip>
      </td>
      <td className="px-4 py-3 text-text-muted">{lastSeen}</td>
      <td className="px-4 py-3">
        {device.kind === 'pending' ? (
          <div className="flex justify-end gap-1.5">
            <Button size="sm" variant="primary" loading={busy} onClick={onApprove}>
              Approve
            </Button>
            <Button size="sm" variant="danger" loading={busy} onClick={onReject}>
              Reject
            </Button>
          </div>
        ) : (
          // Revoke endpoint requires a tunnel session (cookie-based auth).
          // From the localhost /agent dashboard there's no auth context to call
          // it — surface the limitation rather than render a button that 401s.
          <span
            className="text-text-muted text-right block"
            title="Revoke from a tunnel-side browser session"
          >
            —
          </span>
        )}
      </td>
    </tr>
  )
}

function fmtRelative(unixSec: number): string {
  if (!unixSec) return '—'
  const diff = Math.floor(Date.now() / 1000) - unixSec
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}
