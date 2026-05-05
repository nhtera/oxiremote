import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { loadApiKey } from '../../lib/api-client'
import { switchActiveHost } from '../../lib/host-switch-helpers'
import {
  formatRelative,
  removeSavedHost,
  type SavedHost,
} from '../../lib/saved-hosts'

interface Props {
  hosts: SavedHost[]
  onForget: (hostId: string) => void
  onError: (msg: string) => void
}

// Recently-paired devices panel shown above the new-device pair form.
// Tapping a card switches active host via the shared orchestrator. On
// failure the entry is NOT removed automatically — only the explicit
// trash button forgets a host (recoverable: user can re-pair).
export default function SavedHostsPanel({ hosts, onForget, onError }: Props) {
  const navigate = useNavigate()
  const [reconnectingId, setReconnectingId] = useState<string | null>(null)
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})

  if (hosts.length === 0) return null

  const handleReconnect = async (h: SavedHost) => {
    onError('')
    setRowErrors((e) => ({ ...e, [h.host_id]: '' }))
    setReconnectingId(h.host_id)
    const result = await switchActiveHost(h.host_id, navigate)
    setReconnectingId(null)
    if (!result.ok) {
      if (result.error === 'session-expired') {
        setRowErrors((e) => ({ ...e, [h.host_id]: 'Session expired — re-pair or forget' }))
      } else if (result.error === 'mismatch') {
        setRowErrors((e) => ({ ...e, [h.host_id]: 'Host identity mismatch — re-pair' }))
      } else {
        onError('Could not reach that host. Check it is running.')
      }
    }
  }

  const handleForget = (h: SavedHost) => {
    removeSavedHost(h.host_id)
    onForget(h.host_id)
  }

  return (
    <section className="mb-5">
      <h2 className="text-xs font-medium text-text-muted mb-2">Recently paired</h2>
      <div className="space-y-1.5">
        {hosts.map((h) => {
          const hasKey = Boolean(loadApiKey(h.host_id))
          const isReconnecting = reconnectingId === h.host_id
          const rowError = rowErrors[h.host_id]
          return (
            <div key={h.host_id} className="flex flex-col">
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => { if (!isReconnecting) void handleReconnect(h) }}
                  disabled={isReconnecting}
                  className="flex-1 flex items-center gap-2 px-3 py-2 bg-surface-alt border border-border rounded-lg hover:bg-surface-hover transition-colors text-left disabled:opacity-70"
                >
                  {isReconnecting ? (
                    <span
                      aria-hidden="true"
                      className="w-3.5 h-3.5 rounded-full border-2 border-current border-t-transparent animate-spin text-text-muted shrink-0"
                    />
                  ) : (
                    <span
                      className={[
                        'w-1.5 h-1.5 rounded-full shrink-0',
                        hasKey ? 'bg-success' : 'bg-text-muted',
                      ].join(' ')}
                    />
                  )}
                  <div className="flex-1 min-w-0">
                    <div className="text-sm text-text-primary truncate">{h.label}</div>
                    <div className="text-xs text-text-muted truncate">
                      {formatRelative(h.last_seen)}
                      {h.api_key_last4 ? ` · ····${h.api_key_last4}` : ''}
                    </div>
                  </div>
                </button>
                <button
                  type="button"
                  aria-label={`Forget ${h.label}`}
                  onClick={() => handleForget(h)}
                  className="p-1.5 text-text-muted hover:text-danger transition-colors rounded"
                >
                  <svg viewBox="0 0 16 16" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="1.75">
                    <path d="M3 4h10M6 4V3h4v1M5 4v8h6V4H5zm2 2v4m2-4v4" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                </button>
              </div>
              {rowError && (
                <p className="text-xs text-warning px-1 pt-1">{rowError}</p>
              )}
            </div>
          )
        })}
      </div>
      <div className="text-center text-xs text-text-muted mt-3">
        — or pair a new device —
      </div>
    </section>
  )
}
