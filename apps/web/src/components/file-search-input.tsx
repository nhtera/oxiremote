import { useEffect, useRef, useState } from 'react'

export interface SearchHit {
  path: string
  name: string
}

interface Props {
  wsId: number
  onPick: (path: string) => void
}

const DEBOUNCE_MS = 200
const MIN_QUERY = 2

// Filename search input above the file tree. Debounced so we don't punish
// the agent with a fork-per-keystroke. Server caps results at 200 hits +
// walks ≤ 20 dir levels, so this is bounded even on a misclicked /home.
export default function FileSearchInput({ wsId, onPick }: Props) {
  const [q, setQ] = useState('')
  const [hits, setHits] = useState<SearchHit[]>([])
  const [loading, setLoading] = useState(false)
  const [open, setOpen] = useState(false)
  const wrapRef = useRef<HTMLDivElement>(null)

  const trimmed = q.trim()
  const queryActive = trimmed.length >= MIN_QUERY

  useEffect(() => {
    if (!queryActive) return
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLoading(true)
    const id = setTimeout(async () => {
      try {
        const params = new URLSearchParams({ ws_id: String(wsId), q: trimmed })
        const res = await fetch(`/api/files/search?${params}`, { credentials: 'include' })
        if (!res.ok) {
          setHits([])
          return
        }
        setHits(await res.json())
      } catch {
        setHits([])
      } finally {
        setLoading(false)
      }
    }, DEBOUNCE_MS)
    return () => clearTimeout(id)
  }, [trimmed, queryActive, wsId])

  // When the query falls below the threshold we render with no hits and no
  // spinner. Keeping these as derived values avoids a synchronous setState
  // inside the effect body.
  const visibleHits = queryActive ? hits : []
  const spinning = queryActive && loading

  // Close dropdown on outside click — search keeps state, just hides results.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const showDropdown = open && queryActive

  return (
    <div ref={wrapRef} className="relative mb-2">
      <input
        type="text"
        value={q}
        onChange={(e) => {
          setQ(e.target.value)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        placeholder="Search files…"
        aria-label="Search files in workspace"
        className="w-full text-xs bg-surface-alt border border-border rounded-md px-2 py-1.5 outline-none focus:border-accent/60 placeholder:text-text-muted"
      />
      {showDropdown && (
        <div className="absolute z-20 left-0 right-0 mt-1 max-h-60 overflow-auto bg-surface border border-border rounded-md shadow-lg">
          {spinning ? (
            <div className="px-2 py-1.5 text-xs text-text-muted">Searching…</div>
          ) : visibleHits.length === 0 ? (
            <div className="px-2 py-1.5 text-xs text-text-muted">No matches</div>
          ) : (
            <ul>
              {visibleHits.map((h) => (
                <li
                  key={h.path}
                  onClick={() => {
                    onPick(h.path)
                    setOpen(false)
                  }}
                  className="cursor-pointer px-2 py-1.5 text-xs hover:bg-surface-hover truncate"
                  title={h.path}
                >
                  <span className="text-text-primary">{h.name}</span>
                  <span className="text-text-muted ml-2">{h.path}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  )
}
