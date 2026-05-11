// Phase-03 step 6: live stats overlay for the active H.264 desktop session.
// Lazy-loaded — main bundle stays under the 250 KB cap.
//
// Auth note: EventSource can't send Authorization headers, but the global
// fetch interceptor in api-client.ts attaches Bearer + CSRF, so we use
// fetch + ReadableStream and parse `data:` lines ourselves (~30 LOC).
//
// Termination: abort on unmount, on stream end (server emits one final
// snapshot when the desktop session closes), or on transient fetch error
// (component renders a single-line "stats: connecting…" / "stats: closed").
//
// Visual: fixed bottom-right, semi-transparent, monospace, pointer-events
// off so taps fall through to the canvas.

import { useEffect, useRef, useState } from 'react'

interface StatsSnapshot {
  ts_ms: number
  pipeline: string
  remb_kbps: number | null
  bitrate_kbps: number | null
  loss_pct: number | null
  rtt_ms: number | null
  jitter_ms: number | null
  encode_ms_p50: number | null
  encode_ms_p95: number | null
  frames_encoded: number
  keyframes: number
  bgra_queue_depth_peak: number
  sample_queue_depth_peak: number
}

type StreamState = 'connecting' | 'open' | 'closed' | 'error'

interface Props {
  hostId: string
}

export default function DesktopStatsOverlay({ hostId }: Props) {
  const [snap, setSnap] = useState<StatsSnapshot | null>(null)
  const [state, setState] = useState<StreamState>('connecting')
  const abortRef = useRef<AbortController | null>(null)

  useEffect(() => {
    const ac = new AbortController()
    abortRef.current = ac
    let cancelled = false

    void (async () => {
      try {
        const res = await fetch(
          `/api/hosts/${encodeURIComponent(hostId)}/desktop/stats`,
          { signal: ac.signal, credentials: 'include' },
        )
        if (!res.ok || !res.body) {
          if (!cancelled) setState(res.status === 404 ? 'closed' : 'error')
          return
        }
        if (!cancelled) setState('open')
        const reader = res.body.getReader()
        const decoder = new TextDecoder()
        let buf = ''
        while (!cancelled) {
          const { value, done } = await reader.read()
          if (done) {
            if (!cancelled) setState('closed')
            break
          }
          buf += decoder.decode(value, { stream: true })
          // SSE frames are separated by blank lines; each frame is one
          // or more `field: value` lines. We only emit `data:` lines.
          let nl: number
          while ((nl = buf.indexOf('\n\n')) >= 0) {
            const frame = buf.slice(0, nl)
            buf = buf.slice(nl + 2)
            const dataLine = frame
              .split('\n')
              .find((l) => l.startsWith('data:'))
            if (!dataLine) continue
            try {
              const parsed = JSON.parse(dataLine.slice(5).trim()) as StatsSnapshot
              if (!cancelled) setSnap(parsed)
            } catch {
              /* malformed frame — skip */
            }
          }
        }
      } catch (err) {
        if (!cancelled && (err as Error).name !== 'AbortError') setState('error')
      }
    })()

    return () => {
      cancelled = true
      ac.abort()
    }
  }, [hostId])

  return (
    <div
      className="fixed bottom-2 right-2 z-30 pointer-events-none rounded bg-black/70 px-2 py-1 font-mono text-[10px] leading-tight text-white shadow-lg"
      style={{ minWidth: 180 }}
      aria-label="Stream stats"
    >
      {snap ? (
        <>
          <Row label="pipe" value={`${snap.pipeline} · ${snap.frames_encoded}fps`} />
          <Row
            label="rate"
            value={`${formatKbps(snap.bitrate_kbps)} / REMB ${formatKbps(snap.remb_kbps)}`}
          />
          <Row
            label="net"
            value={`rtt ${formatMs(snap.rtt_ms)} · loss ${formatPct(snap.loss_pct)} · jit ${formatMs(snap.jitter_ms)}`}
          />
          <Row
            label="enc"
            value={`p50 ${formatMs(snap.encode_ms_p50)} · p95 ${formatMs(snap.encode_ms_p95)} · idr ${snap.keyframes}`}
          />
          {(snap.bgra_queue_depth_peak > 0 || snap.sample_queue_depth_peak > 0) && (
            <Row
              label="q"
              value={`bgra ${snap.bgra_queue_depth_peak} · sample ${snap.sample_queue_depth_peak}`}
            />
          )}
        </>
      ) : (
        <Row label="stats" value={state} />
      )}
    </div>
  )
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-2">
      <span className="text-white/50 w-9 shrink-0">{label}</span>
      <span className="tabular-nums">{value}</span>
    </div>
  )
}

function formatKbps(v: number | null): string {
  if (v === null) return '—'
  return v >= 1000 ? `${(v / 1000).toFixed(1)}M` : `${v}K`
}

function formatMs(v: number | null): string {
  return v === null ? '—' : `${v}ms`
}

function formatPct(v: number | null): string {
  return v === null ? '—' : `${v.toFixed(1)}%`
}
