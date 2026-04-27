import { useEffect, useState } from 'react'
import { useConfirm } from './ui'

type Phase = 'idle' | 'stopping' | 'stopped'

interface ActiveClient {
  device_id: string
  device_name: string
}

export default function AgentDisconnectButton() {
  const confirm = useConfirm()
  const [phase, setPhase] = useState<Phase>('idle')

  const handleClick = async () => {
    // Fetch active clients to show in confirm dialog.
    let activeClients: ActiveClient[] = []
    try {
      const res = await fetch('/api/agent/state', { cache: 'no-store' })
      if (res.ok) {
        const data: { active_clients?: ActiveClient[] } = await res.json()
        activeClients = data.active_clients ?? []
      }
    } catch {
      // Non-critical; proceed with count = 0.
    }

    const count = activeClients.length
    const nameList = activeClients.map((c) => c.device_name).slice(0, 5)
    const namesStr = nameList.length > 0 ? `: ${nameList.join(', ')}` : ''
    const extra =
      activeClients.length > 5
        ? ` and ${activeClients.length - 5} more`
        : ''

    const clientMsg =
      count > 0
        ? `${count} active client${count !== 1 ? 's' : ''} will be disconnected${namesStr}${extra}.`
        : ''

    const ok = await confirm({
      title: 'Stop the agent?',
      message: [
        'The tunnel will close and all connected devices will disconnect. Restart the agent in your terminal to bring it back.',
        clientMsg,
      ]
        .filter(Boolean)
        .join(' '),
      confirmText: 'Stop agent',
      danger: true,
    })
    if (!ok) return
    setPhase('stopping')
    try {
      await fetch('/api/agent/shutdown', { method: 'POST' })
    } catch {
      // shutdown races the response — network error means it already exited
    }
  }

  // Poll /api/agent/state until the agent stops responding, then show overlay.
  useEffect(() => {
    if (phase !== 'stopping') return
    let cancelled = false
    let elapsed = 0
    const POLL_MS = 400
    const TIMEOUT_MS = 8000
    const interval = window.setInterval(async () => {
      if (cancelled) return
      elapsed += POLL_MS
      try {
        const res = await fetch('/api/agent/state', {
          cache: 'no-store',
          signal: AbortSignal.timeout(800),
        })
        if (!res.ok && res.status >= 500) {
          setPhase('stopped')
          window.clearInterval(interval)
        }
      } catch {
        if (!cancelled) {
          setPhase('stopped')
          window.clearInterval(interval)
        }
      }
      if (elapsed >= TIMEOUT_MS && !cancelled) {
        setPhase('stopped')
        window.clearInterval(interval)
      }
    }, POLL_MS)
    return () => {
      cancelled = true
      window.clearInterval(interval)
    }
  }, [phase])

  return (
    <>
      <button
        type="button"
        onClick={handleClick}
        disabled={phase !== 'idle'}
        className="inline-flex items-center gap-2 rounded-full px-4 py-1.5 text-xs font-semibold border border-danger/40 text-danger hover:bg-danger/10 transition-colors disabled:opacity-60 disabled:cursor-not-allowed"
      >
        <span className="relative inline-flex w-2.5 h-2.5">
          {phase === 'idle' && (
            <span className="absolute inline-flex w-full h-full rounded-full bg-danger opacity-50 animate-ping" />
          )}
          <span className="relative inline-flex w-2.5 h-2.5 rounded-full bg-danger" />
        </span>
        {phase === 'stopping' ? 'Stopping…' : phase === 'stopped' ? 'Stopped' : 'Stop agent'}
      </button>
      {phase === 'stopped' && <AgentStoppedOverlay />}
    </>
  )
}

function AgentStoppedOverlay() {
  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-200 flex items-center justify-center bg-surface/85 backdrop-blur-sm px-4"
    >
      <div className="w-full max-w-md rounded-2xl border border-border bg-surface-alt p-6 md:p-7 text-center shadow-[0_24px_60px_-20px_rgba(0,0,0,0.7)]">
        <div className="inline-flex w-12 h-12 items-center justify-center rounded-full bg-danger/15 border border-danger/30 text-danger mb-4">
          <svg
            viewBox="0 0 24 24"
            className="w-6 h-6"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.25"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <circle cx="12" cy="12" r="9" />
            <path d="M12 8v5" />
            <path d="M12 16h.01" />
          </svg>
        </div>
        <h2 className="text-lg font-semibold text-text-primary tracking-tight">
          Agent stopped
        </h2>
        <p className="mt-2 text-sm text-text-secondary leading-relaxed">
          The OxiRemote process has exited. The tunnel is closed and all
          connected devices have been disconnected.
        </p>
        <div className="mt-5 rounded-lg bg-surface border border-border px-4 py-3 text-left">
          <div className="text-[11px] uppercase tracking-[0.18em] text-text-muted font-medium mb-1.5">
            Restart in your terminal
          </div>
          <code className="font-mono text-sm text-text-primary select-all">
            oxiremote
          </code>
        </div>
        <button
          onClick={() => window.location.reload()}
          className="mt-5 w-full px-4 py-2.5 text-sm font-medium bg-accent/15 text-accent border border-accent/40 rounded-lg hover:bg-accent/25 transition-colors"
        >
          Reload page
        </button>
      </div>
    </div>
  )
}
