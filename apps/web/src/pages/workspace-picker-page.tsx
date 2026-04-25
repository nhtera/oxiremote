import { useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { useWorkspaceStore, type Workspace } from '../state/workspace-store'

export default function WorkspacePickerPage() {
  const { hostId } = useParams<{ hostId: string }>()
  const navigate = useNavigate()
  const currentHostId = useHostStore((s) => s.currentHostId)
  const { list, loading, error, fetchList, setActive, createWorkspace, deleteWorkspace, touch } =
    useWorkspaceStore()

  const [newPath, setNewPath] = useState('')
  const [newLabel, setNewLabel] = useState('')
  const [addError, setAddError] = useState('')
  const [adding, setAdding] = useState(false)

  useEffect(() => {
    fetchList()
  }, [fetchList])

  const pinned = list.filter((w) => w.pinned)
  const recents = list.filter((w) => !w.pinned).slice(0, 5)

  async function onPick(ws: Workspace) {
    if (!currentHostId) return
    setActive(currentHostId, { id: ws.id, path: ws.path, label: ws.label })
    touch(ws.id)
    navigate(`/h/${hostId ?? currentHostId}/files`)
  }

  async function onAdd(e: React.FormEvent) {
    e.preventDefault()
    const path = newPath.trim()
    if (!path) return
    setAdding(true)
    setAddError('')

    // Pre-flight: ask the agent if the path is a real directory before posting.
    try {
      const probe = await fetch('/api/workspace/validate', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path }),
      })
      if (probe.ok) {
        const data = (await probe.json()) as { exists: boolean; kind: string; message?: string }
        if (!data.exists || data.kind !== 'dir') {
          setAdding(false)
          setAddError(data.message || 'Path is not a valid directory.')
          return
        }
      }
    } catch {
      // Network blip — fall through to createWorkspace which has its own error path.
    }

    const res = await createWorkspace(path, newLabel.trim() || undefined)
    setAdding(false)
    if (!res.ok) {
      setAddError(res.error)
      return
    }
    setNewPath('')
    setNewLabel('')
  }

  return (
    <div className="p-4 md:p-6 max-w-2xl">
      <h1 className="text-lg font-semibold mb-1">Workspaces</h1>
      <p className="text-xs text-text-muted mb-4">Pick a folder to browse. Recents stay available for quick access.</p>

      {error && <div className="text-danger text-sm mb-3">{error}</div>}

      {pinned.length > 0 && (
        <section className="mb-5">
          <h2 className="text-xs uppercase tracking-wide text-text-muted mb-2">Pinned</h2>
          <div className="grid gap-2">
            {pinned.map((w) => (
              <WorkspaceRow key={w.id} ws={w} onPick={onPick} onDelete={null} />
            ))}
          </div>
        </section>
      )}

      {recents.length > 0 && (
        <section className="mb-5">
          <h2 className="text-xs uppercase tracking-wide text-text-muted mb-2">Recents</h2>
          <div className="grid gap-2">
            {recents.map((w) => (
              <WorkspaceRow
                key={w.id}
                ws={w}
                onPick={onPick}
                onDelete={() => deleteWorkspace(w.id)}
              />
            ))}
          </div>
        </section>
      )}

      {!loading && list.length === 0 && (
        <div className="text-sm text-text-muted mb-4">No workspaces yet. Add one below.</div>
      )}

      <section className="border border-border rounded-lg bg-surface-alt p-3">
        <h2 className="text-sm font-medium m-0 mb-2">Add folder</h2>
        <form onSubmit={onAdd} className="flex flex-col gap-2">
          <input
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            placeholder="/absolute/path/to/folder"
            className="text-sm bg-surface border border-border rounded-md px-2 py-1.5 focus:outline-none focus:border-accent/50"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
          />
          <input
            value={newLabel}
            onChange={(e) => setNewLabel(e.target.value)}
            placeholder="Optional label"
            className="text-sm bg-surface border border-border rounded-md px-2 py-1.5 focus:outline-none focus:border-accent/50"
          />
          {addError && <div className="text-danger text-xs">{addError}</div>}
          <button
            type="submit"
            disabled={adding || !newPath.trim()}
            className="btn-primary text-sm self-start disabled:opacity-40"
          >
            {adding ? 'Adding…' : 'Add'}
          </button>
        </form>
      </section>
    </div>
  )
}

function WorkspaceRow({
  ws,
  onPick,
  onDelete,
}: {
  ws: Workspace
  onPick: (ws: Workspace) => void
  onDelete: (() => void) | null
}) {
  return (
    <div className="flex items-center gap-2 border border-border rounded-md bg-surface-alt p-2">
      <button
        onClick={() => onPick(ws)}
        className="flex-1 min-w-0 text-left cursor-pointer hover:bg-surface-hover rounded px-1"
      >
        <div className="text-sm text-text-primary truncate">{ws.label}</div>
        <div className="text-xs text-text-muted truncate">{ws.path}</div>
      </button>
      {onDelete && (
        <button
          onClick={onDelete}
          title="Remove workspace"
          className="text-xs text-text-muted hover:text-danger px-2 py-1"
        >
          ✕
        </button>
      )}
    </div>
  )
}
