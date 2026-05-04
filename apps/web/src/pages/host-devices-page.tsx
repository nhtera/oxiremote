// Phone-friendly device manager scoped to the connected host.
// Reads via tunnel-accessible /api/devices (Bearer + CSRF guarded).
// Supports: rename, revoke. No approve/reject (approval requires /api/agent/*
// which is localhost-only; those actions are available in the Host Dashboard).

import { useCallback, useEffect, useRef, useState } from 'react'
import { Navigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { SkeletonLine } from '../components/ui'

type Device = {
  device_id: string
  label: string
  last_seen_at: number
  device_name?: string | null
  platform?: string | null
  last_active_at?: number | null
  user_agent?: string | null
}

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

function fmtRelative(unixSec: number | null | undefined): string {
  if (!unixSec) return '—'
  const diff = Math.floor(Date.now() / 1000) - unixSec
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86_400)}d ago`
}

export default function HostDevicesPage() {
  const { currentHostId } = useHostStore()
  const [devices, setDevices] = useState<Device[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const renameInputRef = useRef<HTMLInputElement>(null)

  const fetchDevices = useCallback(async () => {
    setError('')
    try {
      const res = await fetch('/api/devices', { credentials: 'include' })
      if (res.status === 401) {
        setError('Not authorized. Pair first.')
        return
      }
      if (!res.ok) {
        setError('Failed to load devices')
        return
      }
      const data = (await res.json()) as { devices: Device[] }
      setDevices(data.devices)
    } catch {
      setError('Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    fetchDevices()
  }, [fetchDevices])

  async function revokeDevice(deviceId: string) {
    setBusyId(deviceId)
    setError('')
    try {
      const res = await fetch(`/api/devices/${deviceId}/revoke`, {
        method: 'POST',
        credentials: 'include',
      })
      if (!res.ok) setError('Failed to revoke device')
      else await fetchDevices()
    } finally {
      setBusyId(null)
    }
  }

  function startRename(device: Device) {
    setRenamingId(device.device_id)
    setRenameDraft(device.device_name ?? device.label ?? '')
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
    if (!res.ok) setError('Failed to rename device')
    await fetchDevices()
  }

  if (!currentHostId) return <Navigate to="/login" replace />

  return (
    <div className="p-4 max-w-xl">
      <div className="flex items-center justify-between mb-4 gap-2">
        <h1 className="text-base font-semibold text-text-primary m-0">Devices</h1>
        <button onClick={fetchDevices} className="btn-secondary text-xs">
          Refresh
        </button>
      </div>

      {error && (
        <div className="mb-3 text-danger text-sm">{error}</div>
      )}

      {loading ? (
        <div className="space-y-3" aria-busy="true" aria-label="Loading devices">
          {[1, 2, 3].map((i) => (
            <div key={i} className="border border-border rounded-md p-3 space-y-2">
              <SkeletonLine width="50%" className="h-3" />
              <SkeletonLine width="70%" className="h-2.5" />
            </div>
          ))}
        </div>
      ) : devices.length === 0 ? (
        <div className="text-text-muted text-sm">No trusted devices yet.</div>
      ) : (
        <div className="grid gap-2">
          {devices.map((device) => {
            const isRenaming = renamingId === device.device_id
            const displayName = device.device_name ?? device.label ?? device.device_id.slice(0, 8)
            const badge = platformBadge(device)
            const lastActiveSec = device.last_active_at ? Math.floor(device.last_active_at / 1000) : null

            return (
              <div key={device.device_id} className="flex items-center gap-2 border border-border rounded-md p-3">
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
                      ? <>Active {fmtRelative(lastActiveSec)}</>
                      : <>Last seen {fmtRelative(device.last_seen_at)}</>
                    }
                  </div>
                </div>

                <button
                  onClick={() => revokeDevice(device.device_id)}
                  disabled={busyId === device.device_id}
                  className="btn-danger text-xs py-1 px-2 disabled:opacity-40 shrink-0"
                >
                  {busyId === device.device_id ? 'Revoking…' : 'Revoke'}
                </button>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
