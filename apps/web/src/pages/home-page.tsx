import { useCallback, useEffect, useState } from 'react'
import { Link, Navigate, useLocation } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { Heading } from '../components/ui'
import {
  TerminalIcon,
  GitIcon,
  FilesIcon,
  PreviewIcon,
  RemoteDesktopIcon,
} from '../components/icons'

type IconCmp = (props: { size?: number }) => React.ReactNode

type TerminalSession = { id: string; status: string }
type SessionSummary = { total: number; running: number; latestId?: string | null }
type GitSummary = { staged: number; changed: number }
type Device = {
  device_id: string
  label: string
  last_seen_at: number
}
type DesktopCaps = { available: boolean }

export default function HomePage() {
  const location = useLocation()
  const { currentHostId } = useHostStore()
  const [sessions, setSessions] = useState<SessionSummary>({ total: 0, running: 0, latestId: null })
  const [git, setGit] = useState<GitSummary>({ staged: 0, changed: 0 })
  const [previews, setPreviews] = useState(0)
  const [devices, setDevices] = useState<Device[]>([])
  const [authed, setAuthed] = useState(true)
  const [busyDeviceId, setBusyDeviceId] = useState<string | null>(null)
  const [deviceError, setDeviceError] = useState('')
  const [desktopCaps, setDesktopCaps] = useState<DesktopCaps | null>(null)

  const refreshDevices = useCallback(async () => {
    const res = await fetch('/api/devices', { credentials: 'include' })
    if (res.status === 401) {
      setAuthed(false)
      return
    }
    if (!res.ok) return
    const data = (await res.json()) as { devices: Device[] }
    setDevices(data.devices)
  }, [])

  const loadDashboard = useCallback(async () => {
    try {
      const sessionRes = await fetch('/api/terminal/sessions', { credentials: 'include' })
      if (!sessionRes.ok) throw new Error('unauthorized')
      const sessionData = (await sessionRes.json()) as TerminalSession[]
      setSessions({
        total: sessionData.length,
        running: sessionData.filter((s) => s.status === 'running').length,
        latestId: sessionData.find((s) => s.status === 'running')?.id ?? sessionData[0]?.id ?? null,
      })
    } catch {
      setAuthed(false)
      return
    }

    fetch('/api/git/status', { credentials: 'include' })
      .then((r) => (r.ok ? r.json() : []))
      .then((entries: { index: string; working: string }[]) => {
        setGit({
          staged: entries.filter((e) => e.index !== ' ' && e.index !== '?').length,
          changed: entries.filter((e) => e.working !== ' ' || e.index === '?').length,
        })
      })
      .catch(() => {})

    fetch('/api/previews', { credentials: 'include' })
      .then((r) => (r.ok ? r.json() : []))
      .then((data: unknown[]) => setPreviews(data.length))
      .catch(() => {})

    refreshDevices().catch(() => {})
  }, [refreshDevices])

  useEffect(() => {
    loadDashboard()
  }, [loadDashboard, location.search])

  // Fetch desktop capabilities separately — non-critical, best-effort.
  useEffect(() => {
    if (!currentHostId) return
    fetch(`/api/hosts/${currentHostId}/desktop/capabilities`, { credentials: 'include' })
      .then((r) => (r.ok ? (r.json() as Promise<DesktopCaps>) : null))
      .then((d) => d && setDesktopCaps(d))
      .catch(() => {})
  }, [currentHostId])

  async function revokeDevice(deviceId: string) {
    setBusyDeviceId(deviceId)
    setDeviceError('')
    const res = await fetch(`/api/devices/${deviceId}/revoke`, {
      method: 'POST',
      credentials: 'include',
    })
    setBusyDeviceId(null)
    if (res.status === 401) {
      setAuthed(false)
      return
    }
    if (!res.ok) {
      setDeviceError('Failed to revoke device')
      return
    }
    await refreshDevices()
  }

  function formatLastSeen(ts: number) {
    return new Date(ts * 1000).toLocaleString()
  }

  const hostPrefix = currentHostId ? `/h/${currentHostId}` : ''
  const terminalLink = sessions.latestId
    ? `${hostPrefix}/terminal?session=${encodeURIComponent(sessions.latestId)}`
    : `${hostPrefix}/terminal`

  if (!authed) {
    return <Navigate to="/login" replace />
  }

  return (
    <div className="p-4 md:p-6 max-w-3xl">
      <Heading level={1} className="mb-4">Dashboard</Heading>

      <div className="grid grid-cols-2 md:grid-cols-3 gap-3 mb-6">
        <StatCard label="Terminal" value={`${sessions.running} active`} sub={`${sessions.total} total`} to={terminalLink} />
        <StatCard label="Git" value={`${git.staged} staged`} sub={`${git.changed} changed`} to={`${hostPrefix}/git`} />
        <StatCard label="Previews" value={`${previews} active`} to={`${hostPrefix}/preview`} />
      </div>

      <Heading level={3} className="mb-3">Quick actions</Heading>
      <div className="grid grid-cols-2 sm:grid-cols-5 gap-2 mb-6">
        <QuickAction to={terminalLink} label={sessions.latestId ? 'Resume terminal' : 'New terminal'} Icon={TerminalIcon} />
        <QuickAction to={`${hostPrefix}/git`} label="View changes" Icon={GitIcon} />
        <QuickAction to={`${hostPrefix}/files`} label="Browse files" Icon={FilesIcon} />
        <QuickAction to={`${hostPrefix}/preview`} label="Add preview" Icon={PreviewIcon} />
        <DesktopQuickAction hostId={currentHostId} caps={desktopCaps} />
      </div>

      <div className="border border-border rounded-lg bg-surface-alt p-3">
        <div className="flex items-center justify-between gap-3 mb-2">
          <h2 className="text-sm font-medium m-0">Trusted devices</h2>
          <span className="text-xs text-text-muted">{devices.length} active</span>
        </div>

        {deviceError && <div className="text-danger text-xs mb-2">{deviceError}</div>}

        {devices.length === 0 ? (
          <div className="text-sm text-text-muted">No trusted devices yet.</div>
        ) : (
          <div className="grid gap-2">
            {devices.map((device) => (
              <div key={device.device_id} className="flex items-center gap-2 border border-border rounded-md p-2">
                <div className="flex-1 min-w-0">
                  <div className="text-sm text-text-primary truncate">{device.label}</div>
                  <div className="text-xs text-text-muted truncate">Last seen {formatLastSeen(device.last_seen_at)}</div>
                </div>
                <button
                  onClick={() => revokeDevice(device.device_id)}
                  disabled={busyDeviceId === device.device_id}
                  className="btn-danger text-xs py-1 px-2 disabled:opacity-40"
                >
                  {busyDeviceId === device.device_id ? 'Revoking…' : 'Revoke'}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

function StatCard({ label, value, sub, to }: { label: string; value: string; sub?: string; to: string }) {
  return (
    <Link to={to} className="block bg-surface-alt border border-border rounded-lg p-3 hover:bg-surface-hover transition-colors">
      <div className="text-xs text-text-muted mb-1">{label}</div>
      <div className="text-sm font-medium text-text-primary">{value}</div>
      {sub && <div className="text-xs text-text-muted mt-0.5">{sub}</div>}
    </Link>
  )
}

function QuickAction({ to, label, Icon }: { to: string; label: string; Icon: IconCmp }) {
  return (
    <Link
      to={to}
      className="flex flex-col items-center justify-center gap-1.5 p-3 bg-surface-alt border border-border rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors text-center"
    >
      <Icon size={20} />
      <span className="text-xs leading-tight">{label}</span>
    </Link>
  )
}

function DesktopQuickAction({
  hostId,
  caps,
}: {
  hostId: string | null
  caps: DesktopCaps | null
}) {
  // Not yet loaded or no host — render nothing
  if (!hostId) return null

  const available = caps === null || caps.available // optimistic until loaded

  if (!available) {
    return (
      <span
        title="Screen Recording permission required — open Host Dashboard"
        className="flex flex-col items-center justify-center gap-1.5 p-3 bg-surface-alt border border-border rounded-lg text-text-muted opacity-50 cursor-not-allowed select-none text-center"
      >
        <RemoteDesktopIcon size={20} />
        <span className="text-xs leading-tight">Remote Desktop</span>
      </span>
    )
  }

  return (
    <Link
      to={`/h/${hostId}/desktop`}
      className="flex flex-col items-center justify-center gap-1.5 p-3 bg-surface-alt border border-border rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors text-center"
    >
      <RemoteDesktopIcon size={20} />
      <span className="text-xs leading-tight">Remote Desktop</span>
    </Link>
  )
}
