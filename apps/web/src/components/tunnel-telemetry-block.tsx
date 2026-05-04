import { useEffect, useState } from 'react'
import Sparkline from './sparkline'

// Telemetry from GET /api/agent/tunnel/telemetry
interface TunnelTelemetry {
  uptime_s: number
  last_reconnect_at: number | null
  reconnect_count: number
  bytes_proxied: number
  probe_latencies_p50_ms: number[]
}

const POLL_MS = 5_000

function fmtUptime(s: number): string {
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  return `${h}h ${m}m`
}

function fmtBytes(b: number): string {
  if (b < 1024) return `${b} B`
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`
}

function fmtRelativeSec(sec: number | null): string {
  if (!sec) return '—'
  const diff = Math.floor(Date.now() / 1000) - sec
  if (diff < 5) return 'just now'
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  return `${Math.floor(diff / 3600)}h ago`
}

// Polls /api/agent/tunnel/telemetry every 5 s. Shows uptime, bytes proxied,
// reconnect count, last reconnect time, and a p50 probe-latency sparkline.
// Collapses gracefully when the endpoint is unreachable (tunnel not yet up).
export default function TunnelTelemetryBlock() {
  const [data, setData] = useState<TunnelTelemetry | null>(null)
  const [error, setError] = useState(false)

  useEffect(() => {
    let cancelled = false

    const poll = async () => {
      try {
        const res = await fetch('/api/agent/tunnel/telemetry')
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
        const json = (await res.json()) as TunnelTelemetry
        if (!cancelled) {
          setData(json)
          setError(false)
        }
      } catch {
        if (!cancelled) setError(true)
      }
    }

    void poll()
    const id = window.setInterval(poll, POLL_MS)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [])

  const latencies = data?.probe_latencies_p50_ms ?? []
  const lastLatency = latencies.length > 0 ? latencies[latencies.length - 1] : null

  return (
    <section className="rounded-xl border border-border bg-surface p-4 space-y-3">
      <h2 className="text-[length:var(--text-h3)] font-semibold text-text-primary">
        Tunnel Telemetry
      </h2>

      {error && !data && (
        <div className="text-[length:var(--text-meta)] text-text-muted">
          Telemetry unavailable — tunnel may not be up yet.
        </div>
      )}

      {data && (
        <>
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <Stat label="Uptime" value={fmtUptime(data.uptime_s)} />
            <Stat label="Bytes proxied" value={fmtBytes(data.bytes_proxied)} />
            <Stat label="Reconnects" value={String(data.reconnect_count)} />
            <Stat label="Last reconnect" value={fmtRelativeSec(data.last_reconnect_at)} />
          </div>

          {latencies.length > 0 && (
            <div className="flex items-center gap-3 pt-1">
              <span className="text-[length:var(--text-meta)] text-text-muted shrink-0">
                p50 probe latency
              </span>
              <Sparkline
                values={latencies}
                width={80}
                height={24}
                className="text-accent shrink-0"
              />
              {lastLatency !== null && (
                <span className="text-[length:var(--text-meta)] text-text-secondary">
                  {lastLatency.toFixed(0)} ms
                </span>
              )}
            </div>
          )}
        </>
      )}
    </section>
  )
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg bg-surface-alt border border-border px-3 py-2">
      <div className="text-[10px] uppercase tracking-[0.12em] text-text-muted font-medium mb-0.5">
        {label}
      </div>
      <div className="text-sm font-semibold text-text-primary tabular-nums">
        {value}
      </div>
    </div>
  )
}
