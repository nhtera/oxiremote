import { useCallback, useEffect, useState } from 'react'

interface Preview {
  id: string
  port: number
  label: string
}

export default function PreviewPage() {
  const [previews, setPreviews] = useState<Preview[]>([])
  const [port, setPort] = useState('')
  const [label, setLabel] = useState('')
  const [error, setError] = useState('')

  const fetchPreviews = useCallback(async () => {
    const res = await fetch('/api/previews', { credentials: 'include' })
    if (res.ok) setPreviews(await res.json())
  }, [])

  useEffect(() => { fetchPreviews() }, [fetchPreviews])

  const addPreview = async () => {
    setError('')
    const p = parseInt(port, 10)
    if (!p || p < 1 || p > 65535) {
      setError('Enter a valid port (1–65535)')
      return
    }
    const res = await fetch('/api/previews', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ port: p, label: label || undefined }),
    })
    if (!res.ok) {
      setError(await res.text())
      return
    }
    setPort('')
    setLabel('')
    fetchPreviews()
  }

  const removePreview = async (id: string) => {
    await fetch(`/api/previews/${id}`, { method: 'DELETE', credentials: 'include' })
    fetchPreviews()
  }

  return (
    <div className="p-3 md:p-4 max-w-xl font-mono">
      <h2 className="text-base font-semibold m-0 mb-1">Preview Proxy</h2>
      <p className="text-text-muted text-xs mb-4">
        Proxy a local dev server through the tunnel. Supports HTTP, WebSocket, and SSE.
      </p>

      {error && <div className="text-danger text-sm mb-2">{error}</div>}

      <div className="flex gap-2 mb-4">
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
        <button onClick={addPreview} className="btn-primary text-xs">
          Add
        </button>
      </div>

      {previews.length === 0 ? (
        <div className="text-text-muted text-sm">No active previews</div>
      ) : (
        <div className="grid gap-1">
          {previews.map((p) => (
            <div
              key={p.id}
              className="flex items-center gap-2 py-1.5 border-b border-border"
            >
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
                onClick={() => removePreview(p.id)}
                className="btn-danger text-xs py-0.5 px-2"
              >
                Remove
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
