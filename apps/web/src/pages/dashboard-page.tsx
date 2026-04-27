import { useCallback, useEffect, useRef, useState } from 'react'
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
  // Null for devices paired before the device-identity columns landed.
  device_name?: string | null
  platform?: string | null
  last_active_at?: number | null
  user_agent?: string | null
}
type DesktopCaps = { available: boolean }

/** Short platform badge — prefers the server-set `platform` column, falls
 *  back to parsing `user_agent` for legacy rows where `platform` is NULL. */
function platformBadge(device: Device): string {
  const src = (device.platform ?? device.user_agent ?? '').toLowerCase()
  if (!src) return '?'
  if (src.includes('iphone') || src.includes('ipad') || src.includes('ios')) return 'iOS'
  if (src.includes('android')) return 'Android'
  if (src.includes('mac')) return 'Mac'
  if (src.includes('windows') || src.includes('win')) return 'Win'
  if (src.includes('linux')) return 'Linux'
  return '?'
}

/** Format a Unix-second timestamp as a human-readable relative time. */
function fmtRelative(unixSec: number | null | undefined): string {
  if (!unixSec) return '—'
  const diff = Math.floor(Date.now() / 1000) - unixSec
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86_400)}d ago`
}

export default function DashboardPage() {
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
  // Inline rename state: maps device_id → draft name string while editing.
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const renameInputRef = useRef<HTMLInputElement>(null)

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

  function startRename(device: Device) {
    setRenamingId(device.device_id)
    setRenameDraft(device.device_name ?? device.label ?? '')
    // Focus the input on next tick after render.
    setTimeout(() => renameInputRef.current?.focus(), 0)
  }

  async function commitRename(deviceId: string) {
    setRenamingId(null)
    const trimmed = renameDraft.trim()
    const res = await fetch(`/api/devices/${deviceId}`, {
      method: 'PATCH',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: trimmed || null }),
    })
    if (!res.ok) {
      setDeviceError('Failed to rename device')
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
            {devices.map((device) => {
              const isRenaming = renamingId === device.device_id
              const displayName = device.device_name ?? device.label ?? device.device_id.slice(0, 8)
              const badge = platformBadge(device)
              // last_active_at is unix-ms; fmtRelative expects unix-sec.
              const lastActiveSec = device.last_active_at ? Math.floor(device.last_active_at / 1000) : null
              return (
                <div key={device.device_id} className="flex items-center gap-2 border border-border rounded-md p-2">
                  {/* Platform badge */}
                  <span className="shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded bg-surface-alt border border-border text-text-muted min-w-8 text-center">
                    {badge}
                  </span>

                  <div className="flex-1 min-w-0">
                    {isRenaming ? (
                      <input
                        ref={renameInputRef}
                        className="w-full text-sm bg-surface border border-accent/60 rounded px-1.5 py-0.5 outline-none text-text-primary"
                        value={renameDraft}
                        maxLength={64}
                        onChange={(e) => setRenameDraft(e.target.value)}
                        onBlur={() => void commitRename(device.device_id)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') void commitRename(device.device_id)
                          if (e.key === 'Escape') setRenamingId(null)
                        }}
                      />
                    ) : (
                      <div className="flex items-center gap-1 group">
                        <span className="text-sm text-text-primary truncate">{displayName}</span>
                        <button
                          onClick={() => startRename(device)}
                          className="shrink-0 opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity text-text-muted hover:text-text-primary"
                          title="Rename device"
                          aria-label="Rename device"
                        >
                          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3" aria-hidden="true">
                            <path d="M11.5 2.5a1.414 1.414 0 0 1 2 2L5 13H3v-2L11.5 2.5z" />
                          </svg>
                        </button>
                      </div>
                    )}
                    <div className="text-xs text-text-muted truncate">
                      {lastActiveSec
                        ? <>Active {fmtRelative(lastActiveSec)} · Seen {formatLastSeen(device.last_seen_at)}</>
                        : <>Last seen {formatLastSeen(device.last_seen_at)}</>
                      }
                    </div>
                  </div>

                  <button
                    onClick={() => revokeDevice(device.device_id)}
                    disabled={busyDeviceId === device.device_id}
                    className="btn-danger text-xs py-1 px-2 disabled:opacity-40 shrink-0"
                  >
                    {busyDeviceId === device.device_id ? 'Revoking…' : 'Revoke'}
                  </button>
                </div>
              )
            })}
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
