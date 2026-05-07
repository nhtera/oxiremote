import { useCallback, useEffect, useState } from 'react'

// Phase 02 — operational hint banners for the host dashboard.
//
// Source of truth is GET /api/agent/hints (Localhost-scoped, derived on
// every request). Live updates come from `settings_hint` SSE events on
// `/api/agent/events` — any such event triggers a re-fetch so the banner
// state stays consistent with the underlying derivation. Per-code
// localStorage dismiss state keeps a banner closed until a fresh boot of
// the agent re-emits the same code.

type HintSeverity = 'info' | 'warn' | 'error'

interface Hint {
  code: string
  severity: HintSeverity
  message: string
}

interface HintsResponse {
  hints: Hint[]
}

function dismissKey(code: string): string {
  return `oxi:hint:${code}:dismissed`
}

function isDismissed(code: string): boolean {
  try {
    return localStorage.getItem(dismissKey(code)) === '1'
  } catch {
    return false
  }
}

function setDismissed(code: string): void {
  try {
    localStorage.setItem(dismissKey(code), '1')
  } catch {
    // localStorage unavailable (private mode, quota) — banner returns next mount
  }
}

const TONE_CLASS: Record<HintSeverity, string> = {
  info: 'bg-info/10 border-info/40 text-info',
  warn: 'bg-warning/10 border-warning/40 text-warning',
  error: 'bg-danger/10 border-danger/40 text-danger',
}

function loadDismissedCodes(): Set<string> {
  const next = new Set<string>()
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i)
      if (!k) continue
      if (k.startsWith('oxi:hint:') && k.endsWith(':dismissed') && localStorage.getItem(k) === '1') {
        const code = k.slice('oxi:hint:'.length, -':dismissed'.length)
        next.add(code)
      }
    }
  } catch {
    /* localStorage unavailable */
  }
  return next
}

export default function HintBanners() {
  const [hints, setHints] = useState<Hint[]>([])
  // Lazy initializer reads localStorage on first render — no effect needed.
  const [dismissedCodes, setDismissedCodes] = useState<Set<string>>(loadDismissedCodes)

  const refetch = useCallback(() => {
    fetch('/api/agent/hints')
      .then((r) => (r.ok ? r.json() as Promise<HintsResponse> : { hints: [] }))
      .then((data) => setHints(data.hints ?? []))
      .catch(() => { /* non-critical; next event triggers another try */ })
  }, [])

  useEffect(() => {
    refetch()
  }, [refetch])

  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data) as { type?: string }
        if (ev.type === 'settings_hint') {
          refetch()
        }
      } catch {
        /* drop malformed frames */
      }
    }
    return () => es.close()
  }, [refetch])

  const visible = hints.filter((h) => !dismissedCodes.has(h.code) && !isDismissed(h.code))
  if (visible.length === 0) return null

  return (
    <div className="flex flex-col gap-2">
      {visible.map((h) => (
        <div
          key={h.code}
          className={`flex items-start gap-3 px-4 py-3 rounded-md border text-sm ${TONE_CLASS[h.severity]}`}
          role={h.severity === 'error' ? 'alert' : 'status'}
        >
          <span className="flex-1">{h.message}</span>
          <button
            type="button"
            className="shrink-0 text-xs font-medium opacity-80 hover:opacity-100"
            onClick={() => {
              setDismissed(h.code)
              setDismissedCodes((s) => {
                const next = new Set(s)
                next.add(h.code)
                return next
              })
            }}
          >
            Dismiss
          </button>
        </div>
      ))}
    </div>
  )
}
