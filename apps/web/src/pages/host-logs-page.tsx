// Phone-friendly log viewer scoped to the connected host (tunnel-accessible).
//
// The full agent log stream (/api/agent/events?filter=log) is localhost-only;
// it cannot be reached over the tunnel. This page shows a clear explanation
// and a deep-link to the Host Dashboard where the live stream is available.
// Terminal session state is shown as a substitute live signal.

import { useCallback, useEffect, useState } from 'react'
import { Navigate, Link } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { SkeletonLine } from '../components/ui'

type Session = { id: string; name?: string | null; state: string }

export default function HostLogsPage() {
  const { currentHostId } = useHostStore()
  const [sessions, setSessions] = useState<Session[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const fetchSessions = useCallback(async () => {
    setError('')
    try {
      const res = await fetch('/api/terminal/sessions', { credentials: 'include' })
      if (!res.ok) {
        setError('Not authorized.')
        return
      }
      setSessions(await res.json())
    } catch {
      setError('Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    fetchSessions()
    const t = setInterval(fetchSessions, 10_000)
    return () => clearInterval(t)
  }, [fetchSessions])

  if (!currentHostId) return <Navigate to="/login" replace />

  return (
    <div className="p-4 max-w-xl">
      <h1 className="text-base font-semibold text-text-primary m-0 mb-3">Logs</h1>

      {/* Explain tunnel limitation clearly */}
      <div className="mb-4 p-3 rounded-md border border-border bg-surface-alt text-xs text-text-secondary leading-relaxed">
        <span className="font-medium text-text-primary">Live log stream is localhost-only.</span>
        {' '}Agent logs require a direct connection to the host. Open the Host
        Dashboard from a browser on the same machine, or use{' '}
        <code className="font-mono text-accent bg-surface px-1 rounded">oxiremote ui</code>{' '}
        to launch it.
      </div>

      {/* Terminal sessions as a live activity signal */}
      <div className="mb-2 flex items-center justify-between gap-2">
        <h2 className="text-xs font-medium uppercase tracking-wider text-text-muted m-0">
          Terminal sessions
        </h2>
        <button onClick={fetchSessions} className="btn-secondary text-xs py-0.5 px-2">
          Refresh
        </button>
      </div>

      {error && <div className="text-danger text-sm mb-2">{error}</div>}

      {loading ? (
        <div className="space-y-2" aria-busy="true" aria-label="Loading sessions">
          {[1, 2].map((i) => (
            <div key={i} className="border border-border rounded-md p-2">
              <SkeletonLine width="45%" className="h-3" />
            </div>
          ))}
        </div>
      ) : sessions.length === 0 ? (
        <div className="text-text-muted text-sm">No sessions found.</div>
      ) : (
        <div className="grid gap-1.5">
          {sessions.map((s) => (
            <div key={s.id} className="flex items-center gap-2 border border-border rounded-md px-3 py-2">
              <span
                className={`w-2 h-2 rounded-full shrink-0 ${
                  s.state === 'active' ? 'bg-success' :
                  s.state === 'idle' ? 'bg-warning' : 'bg-text-muted'
                }`}
                title={s.state}
              />
              <span className="text-sm text-text-primary truncate flex-1">
                {s.name ?? s.id.slice(0, 8)}
              </span>
              <span className="text-xs text-text-muted shrink-0">{s.state}</span>
            </div>
          ))}
        </div>
      )}

      <div className="mt-6">
        <Link
          to={`/h/${currentHostId}/dashboard`}
          className="btn-secondary text-xs inline-flex"
        >
          Go to Dashboard
        </Link>
      </div>
    </div>
  )
}
