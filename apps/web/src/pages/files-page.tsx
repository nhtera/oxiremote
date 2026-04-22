import { useCallback, useEffect, useRef, useState } from 'react'

interface FileEntry {
  name: string
  is_dir: boolean
  size: number
  modified: number
}

export default function FilesPage() {
  const [currentPath, setCurrentPath] = useState('')
  const [entries, setEntries] = useState<FileEntry[]>([])
  const [openFile, setOpenFile] = useState<string | null>(null)
  const [content, setContent] = useState('')
  const [dirty, setDirty] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  const fetchList = useCallback(async (path: string) => {
    setError('')
    const q = path ? `?path=${encodeURIComponent(path)}` : ''
    const res = await fetch(`/api/files/list${q}`, { credentials: 'include' })
    if (!res.ok) {
      setError(await res.text())
      return
    }
    setEntries(await res.json())
  }, [])

  useEffect(() => {
    fetchList(currentPath)
  }, [currentPath, fetchList])

  const openDir = (name: string) => {
    setCurrentPath(currentPath ? `${currentPath}/${name}` : name)
    setOpenFile(null)
  }

  const goUp = () => {
    const parts = currentPath.split('/').filter(Boolean)
    parts.pop()
    setCurrentPath(parts.join('/'))
    setOpenFile(null)
  }

  const readFile = async (name: string) => {
    const filePath = currentPath ? `${currentPath}/${name}` : name
    setError('')
    const res = await fetch(`/api/files/read?path=${encodeURIComponent(filePath)}`, {
      credentials: 'include',
    })
    if (!res.ok) {
      setError(await res.text())
      return
    }
    setContent(await res.text())
    setOpenFile(filePath)
    setDirty(false)
  }

  const saveFile = async () => {
    if (!openFile) return
    setSaving(true)
    setError('')
    const res = await fetch('/api/files/write', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: openFile, content }),
    })
    setSaving(false)
    if (!res.ok) {
      setError(await res.text())
      return
    }
    setDirty(false)
  }

  const breadcrumbs = currentPath.split('/').filter(Boolean)

  return (
    <div className="flex flex-col md:flex-row h-[calc(100dvh-8rem)] md:h-dvh font-mono pb-4 gap-2">
      <div className="w-full md:w-72 border-b md:border-b-0 md:border-r border-border overflow-auto p-2 shrink-0 max-h-[36dvh] md:max-h-none">
        <div className="mb-2 text-xs text-text-muted">
          <span
            className="cursor-pointer underline hover:text-text-primary"
            onClick={() => { setCurrentPath(''); setOpenFile(null) }}
          >
            root
          </span>
          {breadcrumbs.map((seg, i) => (
            <span key={i}>
              {' / '}
              <span
                className="cursor-pointer underline hover:text-text-primary"
                onClick={() => {
                  setCurrentPath(breadcrumbs.slice(0, i + 1).join('/'))
                  setOpenFile(null)
                }}
              >
                {seg}
              </span>
            </span>
          ))}
        </div>

        {currentPath && (
          <div
            className="cursor-pointer py-1 text-accent hover:text-accent-hover text-sm"
            onClick={goUp}
          >
            ..
          </div>
        )}

        {entries.map((e) => {
          const fullPath = currentPath ? `${currentPath}/${e.name}` : e.name
          return (
            <div
              key={e.name}
              className={`cursor-pointer py-0.5 text-sm truncate ${
                e.is_dir
                  ? 'text-accent font-semibold'
                  : 'text-text-secondary'
              } ${openFile === fullPath ? 'bg-surface-hover rounded' : ''} hover:text-text-primary`}
              onClick={() => (e.is_dir ? openDir(e.name) : readFile(e.name))}
            >
              {e.is_dir ? '📁 ' : '  '}{e.name}
              {!e.is_dir && (
                <span className="text-text-muted text-xs ml-2">
                  {e.size > 1024 ? `${(e.size / 1024).toFixed(1)}K` : `${e.size}B`}
                </span>
              )}
            </div>
          )
        })}
      </div>

      {/* Right panel — editor */}
      <div className="flex-1 flex flex-col p-2 min-w-0">
        {error && <div className="text-danger text-sm mb-2">{error}</div>}

        {openFile ? (
          <>
            <div className="flex items-center gap-2 mb-2 shrink-0 sticky top-0 bg-surface py-1">
              <span className="flex-1 text-xs text-text-secondary truncate">
                {openFile}{dirty ? ' *' : ''}
              </span>
              <button
                onClick={saveFile}
                disabled={!dirty || saving}
                className="btn-primary text-xs min-h-10 disabled:opacity-40"
              >
                {saving ? 'Saving…' : 'Save'}
              </button>
            </div>
            <textarea
              ref={textareaRef}
              value={content}
              onChange={(e) => { setContent(e.target.value); setDirty(true) }}
              spellCheck={false}
              className="flex-1 font-mono text-sm bg-surface-alt text-text-primary border border-border rounded-md p-2 resize-none focus:outline-none focus:border-accent/50"
              style={{ tabSize: 2 }}
            />
          </>
        ) : (
          <div className="text-text-muted mt-10 text-center text-sm">
            Select a file to edit
          </div>
        )}
      </div>
    </div>
  )
}
