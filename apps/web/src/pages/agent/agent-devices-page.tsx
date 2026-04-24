import { useEffect, useState } from 'react'

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

// Approval status badge colours:
//   pending  → amber/warning
//   approved → green/success
//   rejected → red/danger
function StatusBadge({ status }: { status: string }) {
  const classes: Record<string, string> = {
    pending:
      'bg-warning/15 text-warning border-warning/30',
    approved:
      'bg-success/15 text-success border-success/30',
    rejected:
      'bg-danger/15 text-danger border-danger/30',
  }
  const cls = classes[status] ?? 'bg-surface-alt text-text-muted border-border'
  return (
    <span className={`text-xs font-medium border rounded px-2 py-0.5 ${cls}`}>
      {status}
    </span>
  )
}

export default function AgentDevicesPage() {
  const [devices, setDevices] = useState<AnyDevice[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const fetchDevices = async () => {
    setError(null)
    try {
      // Fetch pending approvals — always present (Phase 02 backend). Bare array response.
      const pendingRes = await fetch('/api/agent/approvals/pending')
      const pending: PendingDevice[] = pendingRes.ok ? await pendingRes.json() : []

      // Fetch full device list. /api/devices returns { devices: [...] } envelope.
      const trustedRes = await fetch('/api/devices', { credentials: 'include' })
      const trusted: TrustedDevice[] = trustedRes.ok
        ? ((await trustedRes.json()) as { devices: TrustedDevice[] }).devices ?? []
        : []

      // Merge: pending devices take priority; skip trusted rows also in pending list
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
    fetchDevices()
  }, [])

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between mb-4">
        <div>
          <h1 className="text-xl font-semibold text-text-primary">Devices</h1>
          <p className="text-sm text-text-muted mt-0.5">
            Paired devices and pending approvals
          </p>
        </div>
        <button
          onClick={fetchDevices}
          className="px-3 py-1.5 text-xs border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
        >
          Refresh
        </button>
      </div>

      {error && (
        <div className="mb-4 px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-sm">
          {error}
        </div>
      )}

      {loading ? (
        <div className="text-sm text-text-muted py-8 text-center">Loading…</div>
      ) : devices.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border p-10 text-center">
          <div className="text-sm text-text-muted">No devices yet.</div>
          <div className="text-xs text-text-muted mt-2">
            Pair a device using the pairing code or a one-time key.
          </div>
        </div>
      ) : (
        <div className="rounded-lg border border-border overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border bg-surface-alt">
                <th className="text-left px-4 py-2.5 text-xs text-text-muted font-medium uppercase tracking-wide">
                  Device ID
                </th>
                <th className="text-left px-4 py-2.5 text-xs text-text-muted font-medium uppercase tracking-wide">
                  IP / Label
                </th>
                <th className="text-left px-4 py-2.5 text-xs text-text-muted font-medium uppercase tracking-wide">
                  User Agent
                </th>
                <th className="text-left px-4 py-2.5 text-xs text-text-muted font-medium uppercase tracking-wide">
                  Status
                </th>
              </tr>
            </thead>
            <tbody>
              {devices.map((d) => (
                <DeviceRow key={d.device_id} device={d} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}

function DeviceRow({ device }: { device: AnyDevice }) {
  const shortId = `${device.device_id.slice(0, 12)}…`
  const status = device.kind === 'pending' ? 'pending' : device.approval_status

  let ipOrLabel = '—'
  let ua = '—'

  if (device.kind === 'pending') {
    ipOrLabel = device.ip
    ua = device.ua_parsed
  } else {
    ipOrLabel = device.label ?? '—'
  }

  return (
    <tr className="border-b border-border last:border-0 hover:bg-surface-alt transition-colors">
      <td className="px-4 py-3 font-mono text-xs text-text-primary" title={device.device_id}>
        {shortId}
      </td>
      <td className="px-4 py-3 text-text-secondary text-xs">{ipOrLabel}</td>
      <td className="px-4 py-3 text-text-muted text-xs max-w-[200px] truncate" title={ua}>
        {ua}
      </td>
      <td className="px-4 py-3">
        <StatusBadge status={status} />
      </td>
    </tr>
  )
}
