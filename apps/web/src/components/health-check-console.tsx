// Streaming console fed by HealthProbe SSE events. Mirrors the TUI's
// "Verifying tunnel…" panel so the operator sees DNS/HEAD attempts ticking
// rather than a silent "Tunnel not ready" placeholder while Cloudflare's
// edge propagates the new subdomain (typically 30-60s).

interface ProbeEntry {
  attempt: number
  status: string
  ok: boolean
  elapsed_ms: number
}

interface Props {
  entries: ProbeEntry[]
  reachable: boolean
}

const VISIBLE = 8

export default function HealthCheckConsole({ entries, reachable }: Props) {
  const visible = entries.slice(-VISIBLE)
  return (
    <div className="font-mono text-xs bg-surface-alt border border-border rounded-md p-3">
      <div className="text-text-muted mb-2">
        {reachable ? 'Tunnel reachable.' : 'Verifying tunnel…'}
      </div>
      {visible.length === 0 ? (
        <div className="text-text-muted">
          Waiting for Cloudflare edge to propagate the subdomain…
        </div>
      ) : (
        <div className="space-y-0.5">
          {visible.map((e) => (
            <div key={e.attempt} className="flex items-center gap-2">
              <span className={`w-3 ${e.ok ? 'text-success' : 'text-text-muted'}`}>
                {e.ok ? '✓' : ' '}
              </span>
              <span className="text-text-muted w-12">#{e.attempt}</span>
              <span className="text-text-primary flex-1 truncate">{e.status}</span>
              <span className="text-text-muted">({e.elapsed_ms}ms)</span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export type { ProbeEntry }
