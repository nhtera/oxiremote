import { useEffect, useState } from 'react'

// Version info from /api/agent/version — fixed for the process lifetime.
// Module-level cache so every AgentHeader/Chip share one fetch.
export interface AgentVersionInfo {
  agent: string
  cloudflared: string
  build_date: string
}

// Shared promise cached at module scope — only one fetch per page load.
let cachedPromise: Promise<AgentVersionInfo | null> | null = null

function fetchVersion(): Promise<AgentVersionInfo | null> {
  if (!cachedPromise) {
    cachedPromise = fetch('/api/agent/version')
      .then((r) => (r.ok ? (r.json() as Promise<AgentVersionInfo>) : null))
      .catch(() => null)
  }
  return cachedPromise
}

export function useAgentVersion(): AgentVersionInfo | null {
  const [info, setInfo] = useState<AgentVersionInfo | null>(null)

  useEffect(() => {
    let cancelled = false
    fetchVersion().then((v) => {
      if (!cancelled) setInfo(v)
    })
    return () => {
      cancelled = true
    }
  }, [])

  return info
}
