import { useState } from 'react'
import { Button, StatusChip, type StatusVariant } from '../ui'

export interface TrustedDevice {
  device_id: string
  label?: string
  user_agent?: string
  approval_status: 'approved' | 'pending' | 'rejected'
  last_seen?: number
  device_name?: string | null
  last_active_at?: number | null
}

export interface PendingDevice {
  device_id: string
  ip: string
  ua_parsed: string
  first_seen: number
}

export type AnyDevice =
  | ({ kind: 'pending' } & PendingDevice)
  | ({ kind: 'trusted' } & TrustedDevice)

export function statusVariant(status: string): StatusVariant {
  if (status === 'approved') return 'online'
  if (status === 'pending') return 'pending'
  if (status === 'rejected') return 'rejected'
  return 'offline'
}

/** Parse a user-agent string into a short platform badge. */
export function parsePlatformBadge(ua: string | undefined | null): string {
  if (!ua) return '?'
  const s = ua.toLowerCase()
  if (s.includes('iphone') || s.includes('ipad')) return 'iOS'
  if (s.includes('android')) return 'Android'
  if (s.includes('mac')) return 'Mac'
  if (s.includes('windows')) return 'Win'
  if (s.includes('linux')) return 'Linux'
  return '?'
}

export function fmtRelative(unixSec: number): string {
  if (!unixSec) return '—'
  const diff = Math.floor(Date.now() / 1000) - unixSec
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}

interface RowProps {
  device: AnyDevice
  busy: boolean
  onApprove: () => void
  onReject: () => void
  onRevoke: () => void
  /** Optional kick — closes the live session/sockets but keeps the device
   *  trusted (API key intact, can reconnect). When undefined the action
   *  is hidden. */
  onDisconnect?: () => void
  onRename: (name: string | null) => Promise<void>
}

export default function AgentDeviceRow({
  device,
  busy,
  onApprove,
  onReject,
  onRevoke,
  onDisconnect,
  onRename,
}: RowProps) {
  const [editing, setEditing] = useState(false)
  const [nameInput, setNameInput] = useState('')
  const [renaming, setRenaming] = useState(false)

  const shortId = `…${device.device_id.slice(-8)}`
  const status = device.kind === 'pending' ? 'pending' : device.approval_status
  const variant = statusVariant(status)

  // Display name: device_name → label → ip (pending) → —
  const displayName =
    device.kind === 'trusted'
      ? (device.device_name ?? device.label ?? '—')
      : device.ip

  // UA: pending uses ua_parsed; trusted uses user_agent
  const ua = device.kind === 'pending' ? device.ua_parsed : (device.user_agent ?? '')
  const platformBadge = parsePlatformBadge(ua)

  // Last active: for trusted prefer last_active_at (ms), fall back to last_seen (s)
  const lastActiveSec =
    device.kind === 'trusted'
      ? (device.last_active_at
          ? Math.floor(device.last_active_at / 1000)
          : (device.last_seen ?? 0))
      : device.first_seen

  const lastActiveStr = lastActiveSec ? fmtRelative(lastActiveSec) : '—'

  const startEdit = () => {
    const current =
      device.kind === 'trusted' ? (device.device_name ?? device.label ?? '') : ''
    setNameInput(current)
    setEditing(true)
  }

  const commitEdit = async () => {
    const val = nameInput.trim()
    setEditing(false)
    setRenaming(true)
    try {
      await onRename(val || null)
    } finally {
      setRenaming(false)
    }
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') commitEdit()
    if (e.key === 'Escape') setEditing(false)
  }

  return (
    <tr className="border-b border-border last:border-0 hover:bg-surface-alt transition-colors">
      {/* Device ID + platform badge */}
      <td className="px-4 py-3" title={device.device_id}>
        <div className="flex items-center gap-1.5">
          <span className="font-mono text-text-primary text-[length:var(--text-meta)]">
            {shortId}
          </span>
          <span className="text-[10px] font-medium px-1.5 py-0.5 rounded bg-surface-alt border border-border text-text-muted">
            {platformBadge}
          </span>
        </div>
      </td>

      {/* Name / IP with inline rename */}
      <td className="px-4 py-3 text-text-secondary">
        {device.kind === 'trusted' && editing ? (
          <input
            autoFocus
            value={nameInput}
            onChange={(e) => setNameInput(e.target.value)}
            onBlur={commitEdit}
            onKeyDown={handleKeyDown}
            maxLength={64}
            className="w-full bg-surface border border-accent/50 rounded px-2 py-0.5 text-[length:var(--text-meta)] text-text-primary focus:outline-none"
          />
        ) : (
          <div className="flex items-center gap-1.5 group">
            <span className={renaming ? 'opacity-50' : ''}>{displayName}</span>
            {device.kind === 'trusted' && (
              <button
                type="button"
                aria-label="Rename device"
                onClick={startEdit}
                className="opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity text-text-muted"
              >
                {/* Pencil icon */}
                <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                  <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
                </svg>
              </button>
            )}
          </div>
        )}
      </td>

      {/* Status chip */}
      <td className="px-4 py-3">
        <StatusChip variant={variant}>{status}</StatusChip>
      </td>

      {/* Last active */}
      <td className="px-4 py-3 text-text-muted text-[length:var(--text-meta)]">
        {lastActiveStr}
      </td>

      {/* Actions */}
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
        ) : device.approval_status === 'approved' ? (
          <div className="flex justify-end gap-1.5">
            {onDisconnect && (
              <Button size="sm" variant="secondary" loading={busy} onClick={onDisconnect}>
                Disconnect
              </Button>
            )}
            <Button size="sm" variant="danger" loading={busy} onClick={onRevoke}>
              Revoke
            </Button>
          </div>
        ) : (
          <span className="text-text-muted text-right block">—</span>
        )}
      </td>
    </tr>
  )
}
