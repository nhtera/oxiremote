import { useEffect, useState } from 'react'
import { useConfirm } from './ui'

type Phase = 'idle' | 'stopping' | 'stopped'

// Stops sharing without exiting the agent process: SIGTERMs cloudflared so
// no new connections succeed, while the host dashboard, settings, and local
// surfaces stay reachable on loopback. Sibling to AgentDisconnectButton (which
// exits the whole process) — colour-coded warning vs. danger to keep them
// visually distinct.
export default function TunnelDisconnectButton() {
  const confirm = useConfirm()
  const [phase, setPhase] = useState<Phase>('idle')
  const [tunnelLive, setTunnelLive] = useState<boolean | null>(null)

  // Poll /api/agent/state for tunnel-url presence so we hide the button when
  // the tunnel is already down (post-restart state, or after a previous
  // disconnect). Every 4 s is plenty for an admin-grade control.
  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      try {
        const res = await fetch('/api/agent/state', { cache: 'no-store' })
        if (!res.ok || cancelled) return
        const data: { tunnel_url?: string | null } = await res.json()
        if (!cancelled) setTunnelLive(Boolean(data.tunnel_url))
      } catch {
        if (!cancelled) setTunnelLive(false)
      }
    }
    void tick()
    const id = window.setInterval(tick, 4000)
    return () => { cancelled = true; window.clearInterval(id) }
  }, [])

  const handleClick = async () => {
    const ok = await confirm({
      title: 'Stop sharing?',
      message:
        'The tunnel will close and all connected devices will disconnect. The agent stays running on this machine; restart sharing requires restarting the agent.',
      confirmText: 'Stop sharing',
      danger: false,
    })
    if (!ok) return
    setPhase('stopping')
    try {
      const res = await fetch('/api/agent/tunnel/disconnect', { method: 'POST' })
      if (res.ok) {
        setPhase('stopped')
        setTunnelLive(false)
      } else {
        setPhase('idle')
      }
    } catch {
      setPhase('idle')
    }
  }

  // Don't render when there's no tunnel to stop (initial probe in flight or
  // tunnel already down). Avoids a no-op button that confuses operators.
  if (tunnelLive === false) return null
  if (tunnelLive === null) return null

  const label =
    phase === 'stopping' ? 'Stopping…' : phase === 'stopped' ? 'Stopped' : 'Stop sharing'
  return (
    <button
      type="button"
      onClick={handleClick}
      disabled={phase !== 'idle'}
      className="inline-flex items-center gap-2 rounded-full px-4 py-1.5 text-xs font-semibold border border-warning/40 text-warning hover:bg-warning/10 transition-colors disabled:opacity-60 disabled:cursor-not-allowed"
    >
      <span className="relative inline-flex w-2.5 h-2.5">
        {phase === 'idle' && (
          <span className="absolute inline-flex w-full h-full rounded-full bg-warning opacity-50 animate-ping" />
        )}
        <span className="relative inline-flex w-2.5 h-2.5 rounded-full bg-warning" />
      </span>
      {label}
    </button>
  )
}
