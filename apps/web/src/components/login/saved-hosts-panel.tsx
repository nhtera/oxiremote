import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../../state/host-store'
import { loadApiKey, storeApiKey } from '../../lib/api-client'
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
// Tapping a card revives the saved api_key (cookie + Bearer) and probes
// /api/me; success → land on /, failure → forget that host and surface
// an error.
export default function SavedHostsPanel({ hosts, onForget, onError }: Props) {
  const navigate = useNavigate()
  const [reconnectingId, setReconnectingId] = useState<string | null>(null)

  if (hosts.length === 0) return null

  const tryReconnect = async (h: SavedHost) => {
    onError('')
    setReconnectingId(h.host_id)
    const key = loadApiKey(h.host_id)
    if (key) storeApiKey(h.host_id, key)
    try {
      const res = await fetch('/api/me', { credentials: 'include' })
      if (res.ok) {
        await useHostStore.getState().fetchHost()
        navigate('/', { replace: true })
        return
      }
    } catch {
      // Network-level error: treat as session expired below.
    } finally {
      setReconnectingId(null)
    }
    removeSavedHost(h.host_id)
    onForget(h.host_id)
    onError('That session expired. Pair this device again.')
  }

  return (
    <section className="mb-5">
      <h2 className="text-xs font-medium text-text-muted mb-2">Recently paired</h2>
      <div className="space-y-1.5">
        {hosts.map((h) => {
          const hasKey = Boolean(loadApiKey(h.host_id))
          const isReconnecting = reconnectingId === h.host_id
          return (
            <button
              key={h.host_id}
              type="button"
              onClick={() => { if (!isReconnecting) void tryReconnect(h) }}
              disabled={isReconnecting}
              className="w-full flex items-center gap-2 px-3 py-2 bg-surface-alt border border-border rounded-lg hover:bg-surface-hover transition-colors text-left disabled:opacity-70"
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
          )
        })}
      </div>
      <div className="text-center text-xs text-text-muted mt-3">
        — or pair a new device —
      </div>
    </section>
  )
}
