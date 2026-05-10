import { useEffect, useState } from 'react'

const STORAGE_KEY = 'oxi:dismissed-named-tunnel-banner'

interface Props {
  /** Per-host scope so dismissing on host A doesn't hide it for host B. */
  hostId: string
}

/**
 * One-line dismissible info banner shown only on Quick Tunnel hosts that
 * haven't been silenced. Quick Tunnel is fine for most users (Phases 1-4
 * handle DNS lag, zombie cloudflared, honest status) — this just points
 * power users at the Named Tunnel upgrade path for a stable hostname.
 *
 * Placement: callers gate on `tunnel_mode === 'quick'` from /api/agent/state.
 */
export default function NamedTunnelBanner({ hostId }: Props) {
  const [dismissed, setDismissed] = useState(true)

  useEffect(() => {
    try {
      const raw = localStorage.getItem(STORAGE_KEY)
      const set = new Set(raw ? (JSON.parse(raw) as string[]) : [])
      setDismissed(set.has(hostId))
    } catch {
      setDismissed(false)
    }
  }, [hostId])

  function dismiss() {
    try {
      const raw = localStorage.getItem(STORAGE_KEY)
      const set = new Set(raw ? (JSON.parse(raw) as string[]) : [])
      set.add(hostId)
      localStorage.setItem(STORAGE_KEY, JSON.stringify([...set]))
    } catch {
      // Ignore — dismissal preference is a nice-to-have, not load-bearing.
    }
    setDismissed(true)
  }

  if (dismissed) return null

  return (
    <div className="px-4 py-3 rounded-md bg-amber-400/10 border border-amber-400/30 text-amber-300 text-sm flex items-center justify-between gap-3">
      <div>
        Quick Tunnels rotate the URL on each restart. For a stable hostname,
        {' '}
        <a
          href="https://github.com/nhtera/oxiremote#named-tunnels-production"
          target="_blank"
          rel="noopener noreferrer"
          className="underline hover:no-underline"
        >
          set up a Named Tunnel
        </a>
        .
      </div>
      <button
        onClick={dismiss}
        className="shrink-0 px-2 py-0.5 text-xs border border-amber-400/30 rounded hover:bg-amber-400/15 transition-colors"
        aria-label="Dismiss banner"
      >
        Dismiss
      </button>
    </div>
  )
}
