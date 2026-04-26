import { useEffect, useState } from 'react'

interface PendingDevice {
  device_id: string
  ip: string
  ua_parsed: string
  first_seen?: number
}

interface Props {
  device: PendingDevice
  onApprove: () => Promise<void>
  onReject: () => Promise<void>
  onClose?: () => void
}

// Modal overlay shown on the /agent dashboard when a device_pending SSE event
// arrives. ua_parsed is rendered via textContent (React default) — no XSS risk.
export default function ApprovalModal({ device, onApprove, onReject, onClose }: Props) {
  const [approving, setApproving] = useState(false)
  const [rejecting, setRejecting] = useState(false)

  // Close on Escape
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose?.()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const handleApprove = async () => {
    setApproving(true)
    try {
      await onApprove()
    } finally {
      setApproving(false)
    }
  }

  const handleReject = async () => {
    setRejecting(true)
    try {
      await onReject()
    } finally {
      setRejecting(false)
    }
  }

  const busy = approving || rejecting
  const shortId = device.device_id.slice(0, 12)

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md bg-surface border border-warning/40 rounded-lg p-5 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="text-warning font-semibold text-sm mb-4">
          Device requesting access
        </div>

        <div className="space-y-2 mb-5">
          <Row label="Device ID" value={`${shortId}…`} mono />
          <Row label="IP Address" value={device.ip} />
          <Row label="User Agent" value={device.ua_parsed} />
        </div>

        <div className="flex gap-2 justify-end">
          <button
            onClick={handleReject}
            disabled={busy}
            className="px-4 py-2 text-sm font-medium border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors disabled:opacity-40"
          >
            {rejecting ? 'Rejecting…' : 'Reject'}
          </button>
          <button
            onClick={handleApprove}
            disabled={busy}
            className="px-4 py-2 text-sm font-medium bg-warning/20 text-warning border border-warning/40 rounded-md hover:bg-warning/30 transition-colors disabled:opacity-40"
          >
            {approving ? 'Approving…' : 'Approve'}
          </button>
        </div>
      </div>
    </div>
  )
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-3 text-sm">
      <span className="text-text-muted shrink-0">{label}</span>
      <span
        className={`text-text-primary truncate text-right ${mono ? 'font-mono' : ''}`}
        title={value}
      >
        {value}
      </span>
    </div>
  )
}
