import { useEffect, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'

// Maximum time to poll before giving up. Matches the OTK TTL pattern —
// 5 min is plenty for a human to reach their computer and tap Approve.
const MAX_POLL_MS = 5 * 60 * 1000
const POLL_INTERVAL_MS = 2000
const TICK_INTERVAL_MS = 1000

type ApprovalStatusResponse = {
  status: 'pending' | 'approved' | 'rejected'
  session_id?: string
}

type Phase = 'pending' | 'rejected' | 'timeout'

interface RouterState {
  session_id?: string
  device_label?: string
}

// "Waiting for Approval" screen shown after OTK login returns 202.
// Polls GET /api/auth/approval-status every 2 s (session cookie attached).
// On approved → /. On rejected/timeout we surface an explicit panel with a
// "Try again" CTA instead of bouncing the user back to /login silently.
export default function ApprovalWaitingPage() {
  const navigate = useNavigate()
  const location = useLocation()
  const routerState = (location.state as RouterState | null) ?? {}

  const [phase, setPhase] = useState<Phase>('pending')
  const [sessionTag, setSessionTag] = useState<string | null>(
    routerState.session_id ? routerState.session_id.slice(-8) : null,
  )
  const [remainingMs, setRemainingMs] = useState(MAX_POLL_MS)

  const stoppedRef = useRef(false)
  const deadlineRef = useRef(Date.now() + MAX_POLL_MS)

  useEffect(() => {
    stoppedRef.current = false
    deadlineRef.current = Date.now() + MAX_POLL_MS
    const controller = new AbortController()

    const poll = async () => {
      if (stoppedRef.current) return
      if (Date.now() >= deadlineRef.current) {
        setPhase('timeout')
        return
      }
      try {
        const res = await fetch('/api/auth/approval-status', {
          credentials: 'include',
          signal: controller.signal,
        })
        if (!res.ok) {
          setPhase('timeout')
          return
        }
        const data: ApprovalStatusResponse = await res.json()
        if (stoppedRef.current) return

        if (data.session_id) setSessionTag(data.session_id.slice(-8))

        if (data.status === 'approved') {
          // Refresh host info before navigating so RootRoute sees the
          // paired host on first render and doesn't bounce back to /welcome.
          await useHostStore.getState().fetchHost()
          navigate('/', { replace: true })
          return
        }
        if (data.status === 'rejected') {
          setPhase('rejected')
          return
        }
        setTimeout(poll, POLL_INTERVAL_MS)
      } catch (err: unknown) {
        if (stoppedRef.current) return
        if (err instanceof DOMException && err.name === 'AbortError') return
        setTimeout(poll, POLL_INTERVAL_MS)
      }
    }

    // Tick the countdown every second so the UI stays accurate.
    const tick = setInterval(() => {
      if (stoppedRef.current) return
      const left = Math.max(0, deadlineRef.current - Date.now())
      setRemainingMs(left)
    }, TICK_INTERVAL_MS)

    const initial = setTimeout(poll, POLL_INTERVAL_MS)

    return () => {
      stoppedRef.current = true
      clearTimeout(initial)
      clearInterval(tick)
      controller.abort()
    }
  }, [navigate])

  if (phase === 'rejected') {
    return (
      <ResultPanel
        tone="danger"
        title="Pairing was declined"
        body="Your computer rejected this pairing request. You can try again with a fresh one-time key."
        primary={{ label: 'Try again', onClick: () => navigate('/login', { replace: true }) }}
      />
    )
  }

  if (phase === 'timeout') {
    return (
      <ResultPanel
        tone="muted"
        title="No response from your computer"
        body="The host didn't respond within five minutes. Generate a new key from the host dashboard and try again."
        primary={{ label: 'Try again', onClick: () => navigate('/login', { replace: true }) }}
      />
    )
  }

  return (
    <div className="min-h-dvh flex items-center justify-center px-6 py-10">
      <div className="w-full max-w-sm">
        <DeviceCard
          deviceLabel={routerState.device_label}
          sessionTag={sessionTag}
        />

        <div className="mt-6 flex flex-col items-center text-center">
          <PulseDot />
          <h1 className="mt-4 text-xl font-semibold text-text-primary">
            Waiting for approval
          </h1>
          <p className="mt-2 text-sm text-text-secondary leading-relaxed max-w-xs">
            Open OxiRemote on your computer. Tap{' '}
            <span className="text-text-primary font-medium">Approve</span> to
            let this device in.
          </p>

          <Countdown remainingMs={remainingMs} />
        </div>

        <button
          onClick={() => navigate('/login', { replace: true })}
          className="w-full mt-6 py-2.5 text-sm border border-border text-text-secondary rounded-md hover:bg-surface-hover hover:text-text-primary transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

interface DeviceCardProps {
  deviceLabel: string | undefined
  sessionTag: string | null
}

function DeviceCard({ deviceLabel, sessionTag }: DeviceCardProps) {
  const ua = typeof navigator !== 'undefined' ? navigator.userAgent : ''
  const browserHint = inferBrowserHint(ua)
  return (
    <div className="rounded-lg border border-border bg-surface-alt p-4">
      <div className="text-xs uppercase tracking-wide text-text-muted mb-2">
        This device
      </div>
      <div className="flex items-center gap-3">
        <div className="shrink-0 w-9 h-9 rounded-md bg-surface flex items-center justify-center text-accent">
          <svg
            viewBox="0 0 24 24"
            className="w-5 h-5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <rect x="5" y="2" width="14" height="20" rx="2" />
            <path d="M11 18h2" />
          </svg>
        </div>
        <div className="min-w-0">
          <div className="text-sm font-medium text-text-primary truncate">
            {deviceLabel || browserHint || 'Unnamed device'}
          </div>
          {sessionTag && (
            <div className="text-xs text-text-muted font-mono mt-0.5">
              session …{sessionTag}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

interface ResultPanelProps {
  tone: 'danger' | 'muted'
  title: string
  body: string
  primary: { label: string; onClick: () => void }
}

function ResultPanel({ tone, title, body, primary }: ResultPanelProps) {
  const accent =
    tone === 'danger'
      ? 'bg-danger/10 border-danger/30 text-danger'
      : 'bg-surface-alt border-border text-text-secondary'
  const icon =
    tone === 'danger' ? (
      <path d="M12 8v4 M12 16h.01 M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
    ) : (
      <path d="M12 6v6l4 2 M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
    )
  return (
    <div className="min-h-dvh flex items-center justify-center px-6 py-10">
      <div className="w-full max-w-sm text-center">
        <div
          className={`inline-flex items-center justify-center w-12 h-12 rounded-full border ${accent} mb-4`}
        >
          <svg
            viewBox="0 0 24 24"
            className="w-6 h-6"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {icon}
          </svg>
        </div>
        <h1 className="text-xl font-semibold text-text-primary">{title}</h1>
        <p className="mt-2 text-sm text-text-secondary leading-relaxed">{body}</p>
        <button
          onClick={primary.onClick}
          className="w-full mt-6 py-3 text-sm font-medium bg-accent/15 text-accent border border-accent/30 rounded-lg hover:bg-accent/25 transition-colors"
        >
          {primary.label}
        </button>
      </div>
    </div>
  )
}

function PulseDot() {
  return (
    <>
      <style>{`
        @keyframes oxi-pulse {
          0%, 100% { transform: scale(1); opacity: 0.7; }
          50%       { transform: scale(1.4); opacity: 0.15; }
        }
        .oxi-pulse-ring {
          position: absolute; inset: 0; border-radius: 9999px;
          background: var(--color-accent);
          animation: oxi-pulse 1.6s ease-out infinite;
        }
      `}</style>
      <div className="relative w-3 h-3 mt-2" role="status" aria-label="Waiting">
        <div className="oxi-pulse-ring" />
        <div className="absolute inset-0 rounded-full bg-accent" />
      </div>
    </>
  )
}

function Countdown({ remainingMs }: { remainingMs: number }) {
  const totalSec = Math.max(0, Math.ceil(remainingMs / 1000))
  const m = Math.floor(totalSec / 60)
  const s = totalSec % 60
  const urgent = remainingMs <= 60_000
  return (
    <div
      className={
        'mt-4 text-xs font-mono ' +
        (urgent ? 'text-warning' : 'text-text-muted')
      }
      aria-live="polite"
    >
      {urgent
        ? `Hurry — waiting expires in ${m}:${s.toString().padStart(2, '0')}`
        : `Waiting expires in ${m}:${s.toString().padStart(2, '0')}`}
    </div>
  )
}

function inferBrowserHint(ua: string): string | null {
  if (!ua) return null
  if (/iPhone|iPad/.test(ua)) return /iPad/.test(ua) ? 'iPad' : 'iPhone'
  if (/Android/.test(ua)) return 'Android phone'
  if (/Mac OS X/.test(ua)) return 'Mac'
  if (/Windows/.test(ua)) return 'Windows PC'
  if (/Linux/.test(ua)) return 'Linux'
  return null
}
