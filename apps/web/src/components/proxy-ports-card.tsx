import { useCallback, useEffect, useMemo, useState } from 'react'

// Localhost-only proxy-port toggle list. Discovered ports come from the
// background lsof/netstat scan; the operator opts each one in or out, and the
// agent persists the allowlist so toggles survive restarts.
//
// Once a port is enabled, anyone with a valid session cookie can hit
// `<tunnel>/proxy/<port>/` from outside.

interface PortsResponse {
  discovered: number[]
  allowed: number[]
}

interface Props {
  tunnelUrl: string | null
}

const POLL_MS = 5_000

export default function ProxyPortsCard({ tunnelUrl }: Props) {
  const [discovered, setDiscovered] = useState<number[]>([])
  const [allowed, setAllowed] = useState<number[]>([])
  const [busyPort, setBusyPort] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState<number | null>(null)

  const refresh = useCallback(async () => {
    try {
      const res = await fetch('/api/agent/proxy/ports')
      if (!res.ok) return
      const data = (await res.json()) as PortsResponse
      setDiscovered(data.discovered ?? [])
      setAllowed(data.allowed ?? [])
    } catch {
      // network blip; next poll recovers
    }
  }, [])

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, POLL_MS)
    return () => clearInterval(t)
  }, [refresh])

  const toggle = async (port: number, enabled: boolean) => {
    setBusyPort(port)
    setError(null)
    try {
      const res = await fetch('/api/agent/proxy/ports', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ port, enabled }),
      })
      if (!res.ok) {
        setError(`Toggle failed (${res.status})`)
        return
      }
      const data = (await res.json()) as { allowed: number[] }
      setAllowed(data.allowed ?? [])
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Toggle failed')
    } finally {
      setBusyPort(null)
    }
  }

  // Union of discovered + allowed so an explicitly-enabled port that briefly
  // stops listening still shows in the list (operator can disable it).
  const ports = useMemo(() => {
    const allowedSet = new Set(allowed)
    const set = new Set<number>([...discovered, ...allowed])
    return Array.from(set)
      .sort((a, b) => a - b)
      .map((p) => ({ port: p, enabled: allowedSet.has(p), live: discovered.includes(p) }))
  }, [discovered, allowed])

  const copyShareUrl = async (port: number) => {
    if (!tunnelUrl) return
    const url = `${tunnelUrl.replace(/\/$/, '')}/proxy/${port}/`
    try {
      await navigator.clipboard.writeText(url)
      setCopied(port)
      setTimeout(() => setCopied((c) => (c === port ? null : c)), 1500)
    } catch {
      // Older browsers / non-secure contexts: silently fall back to no-op.
    }
  }

  return (
    <div className="space-y-2">
      <p className="text-xs text-text-muted">
        Listening ports on this machine. Flip a port on to make it reachable at{' '}
        <code className="text-text-primary">{'<tunnel>/proxy/<port>/'}</code>.
      </p>
      {error && (
        <div className="text-xs text-danger bg-danger/10 border border-danger/30 rounded px-2 py-1">
          {error}
        </div>
      )}
      {ports.length === 0 ? (
        <div className="text-xs text-text-muted">No listening ports detected.</div>
      ) : (
        <div className="grid gap-1">
          {ports.map(({ port, enabled, live }) => (
            <div
              key={port}
              className="flex items-center gap-2 py-1.5 border-b border-border/60 last:border-0"
            >
              <span
                className={`inline-block w-2 h-2 rounded-full ${
                  live ? 'bg-success' : 'bg-text-muted'
                }`}
                title={live ? 'listening' : 'not listening'}
              />
              <span className="flex-1 font-mono text-sm text-text-primary">
                :{port}
              </span>
              {enabled && tunnelUrl && (
                <button
                  type="button"
                  onClick={() => copyShareUrl(port)}
                  className="text-xs text-accent hover:text-accent-hover px-1"
                  title="Copy tunnel URL"
                >
                  {copied === port ? 'copied!' : 'copy URL'}
                </button>
              )}
              <button
                type="button"
                disabled={busyPort === port}
                onClick={() => toggle(port, !enabled)}
                className={`text-xs py-0.5 px-2 rounded ${
                  enabled ? 'btn-primary' : 'btn-secondary'
                } disabled:opacity-50`}
              >
                {enabled ? 'on' : 'off'}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
