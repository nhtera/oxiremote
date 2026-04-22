import { useEffect, useState } from 'react'

type GitStatusEntry = {
  path: string
  index: string
  working: string
}

export default function GitPage() {
  const [entries, setEntries] = useState<GitStatusEntry[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [diffText, setDiffText] = useState<string | null>(null)
  const [diffPath, setDiffPath] = useState<string | null>(null)
  const [commitMsg, setCommitMsg] = useState('')
  const [commitResult, setCommitResult] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  async function refreshStatus() {
    setErr(null)
    const res = await fetch('/api/git/status')
    if (!res.ok) {
      setErr('Not authorized. Pair first at /login.')
      return
    }
    setEntries(await res.json())
  }

  useEffect(() => {
    refreshStatus()
  }, [])

  async function viewDiff(path: string, staged: boolean) {
    setDiffText(null)
    setDiffPath(path)
    const q = new URLSearchParams()
    if (staged) q.set('staged', '1')
    q.set('path', path)
    const res = await fetch(`/api/git/diff?${q}`)
    if (!res.ok) {
      setDiffText('Failed to load diff')
      return
    }
    const text = await res.text()
    setDiffText(text || '(no diff)')
  }

  async function stagePaths(paths: string[]) {
    setLoading(true)
    await fetch('/api/git/stage', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ paths }),
    })
    await refreshStatus()
    setLoading(false)
  }

  async function unstagePaths(paths: string[]) {
    setLoading(true)
    await fetch('/api/git/unstage', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ paths }),
    })
    await refreshStatus()
    setLoading(false)
  }

  async function commit() {
    if (!commitMsg.trim()) return
    setLoading(true)
    setCommitResult(null)
    const res = await fetch('/api/git/commit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: commitMsg }),
    })
    const text = await res.text()
    if (res.ok) {
      setCommitResult(text)
      setCommitMsg('')
      await refreshStatus()
    } else {
      setCommitResult(`Error: ${text}`)
    }
    setLoading(false)
  }

  const staged = entries.filter((e) => e.index !== ' ' && e.index !== '?')
  const unstaged = entries.filter((e) => e.working !== ' ' || e.index === '?')

  return (
    <div className="p-3 md:p-4">
      <div className="flex items-center gap-2 mb-4">
        <h2 className="text-base font-semibold m-0">Git</h2>
        <button onClick={refreshStatus} className="btn-secondary text-xs">
          Refresh
        </button>
      </div>

      {err && <div className="text-danger text-sm mb-3">{err}</div>}

      <div className="flex flex-col md:flex-row gap-4 pb-4">
        {/* File lists */}
        <div className="flex-1 min-w-0">
          <h3 className="text-xs text-text-muted mb-2">Staged ({staged.length})</h3>
          {staged.length === 0 && <div className="text-xs text-text-muted">Nothing staged</div>}
          <div className="grid gap-1.5">
            {staged.map((e) => (
              <FileRow
                key={`s-${e.path}`}
                entry={e}
                onDiff={() => viewDiff(e.path, true)}
                actionLabel="Unstage"
                onAction={() => unstagePaths([e.path])}
                disabled={loading}
              />
            ))}
          </div>

          <h3 className="text-xs text-text-muted mb-2 mt-4">Changes ({unstaged.length})</h3>
          {unstaged.length === 0 && (
            <div className="text-xs text-text-muted">Working tree clean</div>
          )}
          <div className="grid gap-1.5">
            {unstaged.map((e) => (
              <FileRow
                key={`u-${e.path}`}
                entry={e}
                onDiff={() => viewDiff(e.path, false)}
                actionLabel="Stage"
                onAction={() => stagePaths([e.path])}
                disabled={loading}
              />
            ))}
          </div>

          {/* Commit box */}
          <div className="mt-5">
            <textarea
              value={commitMsg}
              onChange={(e) => setCommitMsg(e.target.value)}
              placeholder="Commit message..."
              rows={3}
              className="w-full bg-surface-alt text-text-primary border border-border rounded-md p-2 text-sm font-mono resize-y focus:outline-none focus:border-accent/50"
            />
            <button
              onClick={commit}
              disabled={loading || !commitMsg.trim()}
              className="btn-primary text-xs mt-2 disabled:opacity-40"
            >
              Commit
            </button>
            {commitResult && (
              <pre className="text-xs mt-2 whitespace-pre-wrap text-text-secondary">
                {commitResult}
              </pre>
            )}
          </div>
        </div>

        {/* Diff viewer */}
        <div className="flex-1 min-w-0">
          <h3 className="text-xs text-text-muted mb-2">
            Diff {diffPath ? `— ${diffPath}` : ''}
          </h3>
          <pre className="bg-surface-alt border border-border rounded-lg p-3 text-xs overflow-auto max-h-[500px] whitespace-pre-wrap break-all font-mono text-text-secondary">
            {diffText ?? 'Select a file to view diff'}
          </pre>
        </div>
      </div>
    </div>
  )
}

function FileRow({
  entry,
  onDiff,
  actionLabel,
  onAction,
  disabled,
}: {
  entry: GitStatusEntry
  onDiff: () => void
  actionLabel: string
  onAction: () => void
  disabled: boolean
}) {
  return (
    <div className="flex items-center gap-2 py-1 text-sm">
      <span className="font-mono text-text-muted w-5 text-xs">
        {entry.index}{entry.working}
      </span>
      <span
        onClick={onDiff}
        className="cursor-pointer underline flex-1 min-w-0 truncate text-text-secondary hover:text-text-primary"
      >
        {entry.path}
      </span>
      <button
        onClick={onAction}
        disabled={disabled}
        className="btn-secondary text-xs py-0.5 px-2 disabled:opacity-40"
      >
        {actionLabel}
      </button>
    </div>
  )
}
