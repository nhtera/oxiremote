import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'

// Maximum time to poll before giving up (5 minutes)
const MAX_POLL_MS = 5 * 60 * 1000
const POLL_INTERVAL_MS = 2000

type ApprovalStatusResponse = {
  status: 'pending' | 'approved' | 'rejected'
  session_id?: string
}

// "Waiting for Approval" screen shown after OTK login returns 202.
// Polls GET /api/auth/approval-status every 2s (session cookie attached).
// Navigates to / on approved, /login?error=rejected on rejected.
export default function ApprovalWaitingPage() {
  const navigate = useNavigate()
  const [sessionTag, setSessionTag] = useState<string | null>(null)

  useEffect(() => {
    let stopped = false
    const controller = new AbortController()
    const deadline = Date.now() + MAX_POLL_MS

    const poll = async () => {
      if (stopped) return
      if (Date.now() >= deadline) {
        // Timed out — send back to login
        navigate('/login', { replace: true })
        return
      }

      try {
        const res = await fetch('/api/auth/approval-status', {
          credentials: 'include',
          signal: controller.signal,
        })
        if (!res.ok) {
          // Session gone or server error — back to login
          if (!stopped) navigate('/login', { replace: true })
          return
        }
        const data: ApprovalStatusResponse = await res.json()
        if (stopped) return

        if (data.session_id) setSessionTag(data.session_id.slice(-8))

        if (data.status === 'approved') {
          navigate('/', { replace: true })
          return
        }
        if (data.status === 'rejected') {
          navigate('/login?error=rejected', { replace: true })
          return
        }
        // Still pending — schedule next poll
        setTimeout(poll, POLL_INTERVAL_MS)
      } catch (err: unknown) {
        if (stopped) return
        // AbortError means component unmounted — silently stop
        if (err instanceof DOMException && err.name === 'AbortError') return
        // Network error — retry
        setTimeout(poll, POLL_INTERVAL_MS)
      }
    }

    // Start first poll after one interval so the UI renders first
    const initial = setTimeout(poll, POLL_INTERVAL_MS)

    return () => {
      stopped = true
      clearTimeout(initial)
      controller.abort()
    }
  }, [navigate])

  return (
    <div className="min-h-dvh flex items-center justify-center p-6">
      <div className="flex flex-col items-center gap-5 max-w-sm text-center">
        <ArcSpinner />

        <div>
          <h1 className="text-xl font-semibold text-text-primary">Waiting for Approval</h1>
          <p className="mt-2 text-sm text-text-secondary leading-relaxed">
            The host needs to approve this device. Check the terminal or host dashboard.
          </p>
          {sessionTag && (
            <p className="mt-3 text-xs font-mono text-text-muted" aria-label="Session identifier">
              Session: {sessionTag}
            </p>
          )}
        </div>

        <button
          onClick={() => navigate('/login')}
          className="px-4 py-2 text-sm border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

// CSS-animated orange arc spinner
function ArcSpinner() {
  return (
    <>
      <style>{`
        @keyframes oxi-spin {
          to { transform: rotate(360deg); }
        }
        .oxi-arc-spinner {
          width: 48px;
          height: 48px;
          border: 3px solid transparent;
          border-top-color: var(--color-warning);
          border-right-color: var(--color-warning);
          border-radius: 50%;
          animation: oxi-spin 0.9s linear infinite;
        }
      `}</style>
      <div className="oxi-arc-spinner" role="status" aria-label="Waiting for approval" />
    </>
  )
}
