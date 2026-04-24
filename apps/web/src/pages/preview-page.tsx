import { useCallback, useEffect, useMemo, useState } from 'react'

import PreviewShareModal from '../components/preview-share-modal'
import { apiFetch } from '../lib/transport'

type Health = 'ok' | 'down' | 'unknown'

interface SavedPreview {
  id: string
  port: number
  label: string
  created_at: number
  health: Health
  health_checked_at: number | null
}

interface ShareInfo {
  id: string
  url: string
  expires_at: number
}

const POLL_MS = 10_000

const healthClass: Record<Health, string> = {
  ok: 'bg-success',
  down: 'bg-danger',
  unknown: 'bg-text-muted',
}

export default function PreviewPage() {
  const [discovered, setDiscovered] = useState<number[]>([])
  const [saved, setSaved] = useState<SavedPreview[]>([])
  const [port, setPort] = useState('')
  const [label, setLabel] = useState('')
  const [error, setError] = useState('')
  const [shareInfo, setShareInfo] = useState<ShareInfo | null>(null)

  const refresh = useCallback(async () => {
    try {
      const [localsRes, savedRes] = await Promise.all([
        apiFetch('/api/local-sites'),
        apiFetch('/api/previews'),
      ])
      if (localsRes.ok) setDiscovered(await localsRes.json())
      if (savedRes.ok) setSaved(await savedRes.json())
    } catch {
      // network blip; keep previous state, next poll recovers
    }
  }, [])

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, POLL_MS)
    return () => clearInterval(t)
  }, [refresh])

  const savedPorts = useMemo(() => new Set(saved.map((p) => p.port)), [saved])
  const discoveredNew = useMemo(
    () => discovered.filter((p) => !savedPorts.has(p)),
    [discovered, savedPorts],
  )

  const addPreview = async (p: number, lbl?: string) => {
    setError('')
    if (!p || p < 1 || p > 65535) {
      setError('Enter a valid port (1–65535)')
      return
    }
    const res = await apiFetch('/api/previews', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ port: p, label: lbl || undefined }),
    })
    if (!res.ok) {
      setError(await res.text())
      return
    }
    setPort('')
    setLabel('')
    refresh()
  }

  const removePreview = async (id: string) => {
    await apiFetch(`/api/previews/${id}`, { method: 'DELETE' })
    refresh()
  }

  const share = async (id: string) => {
    setError('')
    const res = await apiFetch(`/api/previews/${id}/share`, { method: 'POST' })
    if (!res.ok) {
      setError(await res.text())
      return
    }
    const data = (await res.json()) as { url: string; expires_at: number }
    setShareInfo({ id, url: data.url, expires_at: data.expires_at })
  }

  return (
    <div className="p-3 md:p-4 max-w-xl font-mono">
      <h2 className="text-base font-semibold m-0 mb-1">Preview Proxy</h2>
      <p className="text-text-muted text-xs mb-4">
        Proxy a local dev server through the tunnel. Supports HTTP, WebSocket, and SSE.
      </p>

      {error && <div className="text-danger text-sm mb-2">{error}</div>}

      <section className="mb-6">
        <h3 className="text-xs uppercase tracking-wide text-text-muted m-0 mb-2">
          Discovered ({discoveredNew.length})
        </h3>
        {discoveredNew.length === 0 ? (
          <div className="text-text-muted text-xs">No new listening ports.</div>
        ) : (
          <div className="grid gap-1">
            {discoveredNew.map((p) => (
              <div
                key={p}
                className="flex items-center gap-2 py-1.5 border-b border-border"
              >
                <span className="flex-1 text-text-secondary text-sm">
                  <span className="text-text-muted">:</span>
                  {p}
                </span>
                <button
                  onClick={() => addPreview(p)}
                  className="btn-primary text-xs py-0.5 px-2"
                >
                  Save
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="mb-6">
        <h3 className="text-xs uppercase tracking-wide text-text-muted m-0 mb-2">
          Add manually
        </h3>
        <div className="flex gap-2">
          <input
            type="number"
            placeholder="Port"
            value={port}
            onChange={(e) => setPort(e.target.value)}
            className="w-20 px-2 py-1.5 text-sm bg-surface-alt border border-border rounded-md text-text-primary font-mono focus:outline-none focus:border-accent/50"
          />
          <input
            placeholder="Label (optional)"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            className="flex-1 px-2 py-1.5 text-sm bg-surface-alt border border-border rounded-md text-text-primary font-mono focus:outline-none focus:border-accent/50"
          />
          <button
            onClick={() => addPreview(parseInt(port, 10), label)}
            className="btn-primary text-xs"
          >
            Add
          </button>
        </div>
      </section>

      <section>
        <h3 className="text-xs uppercase tracking-wide text-text-muted m-0 mb-2">
          Saved ({saved.length})
        </h3>
        {saved.length === 0 ? (
          <div className="text-text-muted text-xs">No saved previews.</div>
        ) : (
          <div className="grid gap-1">
            {saved.map((p) => (
              <div
                key={p.id}
                className="flex items-center gap-2 py-1.5 border-b border-border"
              >
                <span
                  className={`inline-block w-2 h-2 rounded-full ${healthClass[p.health]}`}
                  title={`health: ${p.health}`}
                  aria-label={`health ${p.health}`}
                />
                <span className="flex-1 text-text-secondary text-sm truncate">
                  {p.label}{' '}
                  <span className="text-text-muted text-xs">:{p.port}</span>
                </span>
                <a
                  href={`/preview/${p.id}/`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent hover:text-accent-hover text-xs"
                >
                  Open
                </a>
                <button
                  onClick={() => share(p.id)}
                  className="btn-secondary text-xs py-0.5 px-2"
                >
                  Share
                </button>
                <button
                  onClick={() => removePreview(p.id)}
                  className="btn-danger text-xs py-0.5 px-2"
                >
                  Remove
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      {shareInfo && (
        <PreviewShareModal info={shareInfo} onClose={() => setShareInfo(null)} />
      )}
    </div>
  )
}
