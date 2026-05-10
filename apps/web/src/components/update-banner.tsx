import { useEffect, useState } from 'react'

interface UpdateInfo {
  available: boolean
  latest_version: string | null
}

const POLL_MS = 60_000

// Yellow banner that appears when the agent's self-update check finds a newer
// release. Polls /api/agent/update-check every 60 s — lightweight, no
// server-push needed. Clicking "Update" navigates to /agent/settings#about.
export default function UpdateBanner() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    let cancelled = false
    let intervalId: number | null = null

    const poll = async () => {
      try {
        const res = await fetch('/api/agent/update-check')
        // 404 means this agent build doesn't expose the route — stop
        // polling so we don't spam the DevTools network panel forever.
        // (The endpoint may be added in a later release; the banner is
        // best-effort and lazy-discovers it on next page load.)
        if (res.status === 404) {
          if (intervalId !== null) {
            window.clearInterval(intervalId)
            intervalId = null
          }
          return
        }
        if (!res.ok) return
        const data = (await res.json()) as UpdateInfo
        if (!cancelled && data.available) {
          setUpdate(data)
        }
      } catch {
        // Non-critical — update check is best-effort.
      }
    }

    void poll()
    intervalId = window.setInterval(poll, POLL_MS)
    return () => {
      cancelled = true
      if (intervalId !== null) {
        window.clearInterval(intervalId)
      }
    }
  }, [])

  if (!update?.available || dismissed) return null

  return (
    <div
      role="alert"
      className="flex items-center gap-3 px-4 py-2.5 bg-warning/10 border-b border-warning/30 text-warning text-[length:var(--text-meta)]"
    >
      <svg
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="w-4 h-4 shrink-0"
        aria-hidden="true"
      >
        <path d="M8 2v6M8 11v1" />
        <circle cx="8" cy="8" r="6.5" />
      </svg>
      <span className="flex-1">
        Update available
        {update.latest_version ? `: v${update.latest_version}` : ''}. Run{' '}
        <code className="font-mono text-[length:var(--text-mono)]">oxiremote update</code> in your
        terminal.
      </span>
      <a
        href="/agent/settings"
        className="shrink-0 underline underline-offset-2 hover:opacity-80 transition-opacity"
      >
        About
      </a>
      <button
        type="button"
        onClick={() => setDismissed(true)}
        aria-label="Dismiss update notice"
        className="shrink-0 opacity-60 hover:opacity-100 transition-opacity"
      >
        <svg
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          className="w-4 h-4"
          aria-hidden="true"
        >
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
      </button>
    </div>
  )
}
