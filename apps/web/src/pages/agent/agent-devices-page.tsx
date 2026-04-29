import { useEffect, useMemo, useState } from 'react'
import { StateView } from '../../components/ui'
import AutoApproveToggle from '../../components/auto-approve-toggle'
import AgentDeviceRow, {
  type AnyDevice,
  type PendingDevice,
  type TrustedDevice,
} from '../../components/agent/agent-device-row'

// Devices page — live SSE subscription replaces the manual Refresh button.
// Events device_pending / device_approved / device_rejected trigger a refetch.
// Each row supports: platform icon, inline rename, last_active relative time,
// Revoke button (approved trusted devices only).

export default function AgentDevicesPage() {
  const [devices, setDevices] = useState<AnyDevice[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)
  const [autoApprove, setAutoApprove] = useState(false)

  const fetchDevices = async () => {
    setError(null)
    try {
      const [pendingRes, trustedRes] = await Promise.all([
        fetch('/api/agent/approvals/pending'),
        fetch('/api/agent/devices'),
      ])
      const pending: PendingDevice[] = pendingRes.ok ? await pendingRes.json() : []
      const trusted: TrustedDevice[] = trustedRes.ok ? await trustedRes.json() : []

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

  // Initial load + auto-approve state.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    fetchDevices()
    fetch('/api/agent/state')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => { if (data) setAutoApprove(Boolean(data.auto_approve)) })
      .catch(() => { /* non-critical */ })
  // fetchDevices is stable (defined outside component); eslint-disable is intentional.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Live SSE — refetch on device lifecycle events.
  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev: { type: string } = JSON.parse(msg.data)
        if (
          ev.type === 'device_pending' ||
          ev.type === 'device_approved' ||
          ev.type === 'device_rejected'
        ) {
          // Refetch full list so names/statuses stay in sync.
          fetchDevices()
        }
      } catch {
        // drop malformed frames
      }
    }
    return () => es.close()
  // eslint-disable-next-line react-hooks/exhaustive-deps
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

  async function revoke(id: string) {
    setBusyId(id)
    try {
      await fetch(`/api/agent/devices/${id}/revoke`, { method: 'POST' })
      await fetchDevices()
    } finally {
      setBusyId(null)
    }
  }

  async function disconnectDevice(id: string) {
    setBusyId(id)
    try {
      await fetch(`/api/agent/devices/${id}/disconnect`, { method: 'POST' })
      await fetchDevices()
    } finally {
      setBusyId(null)
    }
  }

  async function rename(id: string, name: string | null) {
    await fetch(`/api/agent/devices/${id}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    await fetchDevices()
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
            Paired devices and pending approvals — updates live
          </p>
        </div>
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

      <div className="rounded-lg border border-border bg-surface px-4 py-3 mb-3 flex items-center justify-between gap-4">
        <div>
          <div className="text-sm font-medium text-text-primary">Auto-approve</div>
          <div className="text-xs text-text-muted mt-0.5">
            {autoApprove
              ? 'New devices are approved automatically'
              : 'New devices require manual approval'}
          </div>
        </div>
        <AutoApproveToggle enabled={autoApprove} onChange={setAutoApprove} />
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
                <Th>Name</Th>
                <Th>Status</Th>
                <Th>Last active</Th>
                <Th className="text-right">Actions</Th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((d) => (
                <AgentDeviceRow
                  key={d.device_id}
                  device={d}
                  busy={busyId === d.device_id}
                  onApprove={() => approve(d.device_id)}
                  onReject={() => reject(d.device_id)}
                  onRevoke={() => revoke(d.device_id)}
                  onDisconnect={() => disconnectDevice(d.device_id)}
                  onRename={(name) => rename(d.device_id, name)}
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
