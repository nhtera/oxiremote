import { Suspense, lazy, useCallback, useEffect, useRef, useState } from 'react'
import { Navigate, useParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { useWorkspaceStore } from '../state/workspace-store'
import FileTree, { type FileEntry } from '../components/file-tree'
import FileActionsMenu, { type FileAction } from '../components/file-actions-menu'
import { useConfirm, usePrompt } from '../components/ui'

const CodeEditor = lazy(() => import('../components/code-editor'))

interface MenuState {
  entry: FileEntry | null
  x: number
  y: number
}

export default function FilesPage() {
  const { hostId } = useParams<{ hostId: string }>()
  const currentHostId = useHostStore((s) => s.currentHostId)
  const activeMap = useWorkspaceStore((s) => s.active)
  const clearActive = useWorkspaceStore((s) => s.clearActive)
  const touch = useWorkspaceStore((s) => s.touch)

  const activeWs = currentHostId ? activeMap[currentHostId] : undefined

  const [currentPath, setCurrentPath] = useState('')
  const [entries, setEntries] = useState<FileEntry[]>([])
  const [openFile, setOpenFile] = useState<string | null>(null)
  const [content, setContent] = useState('')
  const [openedMtime, setOpenedMtime] = useState<number>(0)
  const [dirty, setDirty] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [menu, setMenu] = useState<MenuState | null>(null)
  const [conflict, setConflict] = useState<{ mtime: number } | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const uploadTargetDirRef = useRef<string>('')
  const confirm = useConfirm()
  const prompt = usePrompt()

  const wsId = activeWs?.id

  const fetchList = useCallback(
    async (path: string) => {
      if (wsId == null) return
      setError('')
      const params = new URLSearchParams()
      params.set('ws_id', String(wsId))
      if (path) params.set('path', path)
      const res = await fetch(`/api/files/list?${params}`, { credentials: 'include' })
      if (!res.ok) {
        setError(await res.text())
        return
      }
      setEntries(await res.json())
    },
    [wsId],
  )

  useEffect(() => {
    if (wsId != null) {
      void fetchList(currentPath)
      touch(wsId)
    }
  }, [wsId, currentPath, fetchList, touch])

  if (!currentHostId) return <Navigate to="/login" replace />
  if (!activeWs) return <Navigate to={`/h/${hostId ?? currentHostId}/workspaces`} replace />

  const openDir = (name: string) => {
    setCurrentPath(currentPath ? `${currentPath}/${name}` : name)
    setOpenFile(null)
    setDirty(false)
  }

  const goUp = () => {
    const parts = currentPath.split('/').filter(Boolean)
    parts.pop()
    setCurrentPath(parts.join('/'))
    setOpenFile(null)
    setDirty(false)
  }

  const openFileByName = async (name: string) => {
    const filePath = currentPath ? `${currentPath}/${name}` : name
    setError('')
    const params = new URLSearchParams()
    params.set('ws_id', String(wsId))
    params.set('path', filePath)
    const res = await fetch(`/api/files/read?${params}`, { credentials: 'include' })
    if (!res.ok) {
      if (res.status === 415) {
        setError('Binary file — use Download')
      } else if (res.status === 413) {
        setError('File too large to open (use Download)')
      } else {
        setError(await res.text())
      }
      return
    }
    const data = (await res.json()) as { content: string; modified: number; size: number }
    setContent(data.content)
    setOpenedMtime(data.modified)
    setOpenFile(filePath)
    setDirty(false)
    setConflict(null)
  }

  const saveFile = async (overwrite = false) => {
    if (!openFile || wsId == null) return
    setSaving(true)
    setError('')
    const res = await fetch('/api/files/write', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        path: openFile,
        content,
        ws_id: wsId,
        if_mtime: overwrite ? undefined : openedMtime,
      }),
    })
    setSaving(false)
    if (res.status === 409) {
      const data = (await res.json()) as { current_mtime: number }
      setConflict({ mtime: data.current_mtime })
      return
    }
    if (!res.ok) {
      setError(await res.text())
      return
    }
    const data = (await res.json()) as { modified: number; size: number }
    setOpenedMtime(data.modified)
    setDirty(false)
    setConflict(null)
    void fetchList(currentPath)
  }

  const reloadAfterConflict = async () => {
    if (!openFile) return
    const parts = openFile.split('/')
    const name = parts.pop() ?? ''
    const dir = parts.join('/')
    setCurrentPath(dir)
    setConflict(null)
    await openFileByName(name)
  }

  const onContextMenu = (entry: FileEntry | null, x: number, y: number) => {
    setMenu({ entry, x, y })
  }

  const runAction = async (action: FileAction) => {
    const entry = menu?.entry ?? null
    setMenu(null)
    if (wsId == null) return
    switch (action) {
      case 'new-file':
      case 'new-folder': {
        const name = await prompt({
          title: action === 'new-file' ? 'New file' : 'New folder',
          placeholder: action === 'new-file' ? 'filename.ext' : 'folder-name',
          confirmText: 'Create',
        })
        if (!name) return
        const dir = entry && entry.is_dir ? joinPath(currentPath, entry.name) : currentPath
        const path = joinPath(dir, name)
        const res = await fetch('/api/files/create', {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path, kind: action === 'new-file' ? 'file' : 'dir', ws_id: wsId }),
        })
        if (!res.ok) setError(await res.text())
        else void fetchList(currentPath)
        return
      }
      case 'rename': {
        if (!entry) return
        const current = joinPath(currentPath, entry.name)
        const next = await prompt({
          title: 'Rename',
          message: entry.name,
          initial: entry.name,
          confirmText: 'Rename',
        })
        if (!next || next === entry.name) return
        const to = joinPath(currentPath, next)
        const res = await fetch('/api/files/rename', {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ from: current, to, ws_id: wsId }),
        })
        if (!res.ok) setError(await res.text())
        else {
          if (openFile === current) {
            setOpenFile(to)
          }
          void fetchList(currentPath)
        }
        return
      }
      case 'delete': {
        if (!entry) return
        const ok = await confirm({
          title: `Delete ${entry.name}?`,
          message: entry.is_dir ? 'The folder and its contents will be removed.' : undefined,
          confirmText: 'Delete',
          danger: true,
        })
        if (!ok) return
        const p = joinPath(currentPath, entry.name)
        const res = await fetch('/api/files/delete', {
          method: 'POST',
          credentials: 'include',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: p, ws_id: wsId }),
        })
        if (!res.ok) setError(await res.text())
        else {
          if (openFile === p) setOpenFile(null)
          void fetchList(currentPath)
        }
        return
      }
      case 'upload': {
        const dir = entry && entry.is_dir ? joinPath(currentPath, entry.name) : currentPath
        uploadTargetDirRef.current = dir
        fileInputRef.current?.click()
        return
      }
      case 'download': {
        if (!entry) return
        const p = joinPath(currentPath, entry.name)
        const params = new URLSearchParams()
        params.set('ws_id', String(wsId))
        params.set('path', p)
        window.location.href = `/api/files/download?${params}`
        return
      }
    }
  }

  const onFilePicked = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (!file || wsId == null) return
    const form = new FormData()
    form.append('dir', uploadTargetDirRef.current)
    form.append('file', file, file.name)
    const res = await fetch(`/api/files/upload?ws_id=${wsId}`, {
      method: 'POST',
      credentials: 'include',
      body: form,
    })
    if (!res.ok) setError(await res.text())
    else void fetchList(currentPath)
  }

  const breadcrumbs = currentPath.split('/').filter(Boolean)

  return (
    <div className="flex flex-col md:flex-row h-[calc(100dvh-8rem)] md:h-dvh font-mono pb-4 gap-2">
      <div className="w-full md:w-72 border-b md:border-b-0 md:border-r border-border p-2 shrink-0 max-h-[36dvh] md:max-h-none flex flex-col">
        <div className="mb-1 flex items-center gap-1">
          <button
            onClick={() => clearActive(currentHostId)}
            className="text-xs text-text-muted hover:text-text-primary truncate max-w-[14rem]"
            title={activeWs.path}
          >
            {activeWs.label} ⇄
          </button>
        </div>
        <div className="mb-2 text-xs text-text-muted truncate">
          <span
            className="cursor-pointer underline hover:text-text-primary"
            onClick={() => {
              setCurrentPath('')
              setOpenFile(null)
            }}
          >
            /
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
        <button
          onClick={(e) => onContextMenu(null, e.clientX, e.clientY)}
          className="text-xs text-accent hover:text-accent-hover self-start mb-2"
        >
          + actions
        </button>
        <div className="flex-1 min-h-0">
          <FileTree
            currentPath={currentPath}
            entries={entries}
            selectedPath={openFile}
            onOpenDir={openDir}
            onOpenFile={openFileByName}
            onGoUp={goUp}
            onContextMenu={onContextMenu}
          />
        </div>
      </div>

      {/* Right panel — editor */}
      <div className="flex-1 flex flex-col p-2 min-w-0">
        {error && <div className="text-danger text-sm mb-2">{error}</div>}

        {conflict && (
          <div className="border border-border rounded-md p-2 mb-2 text-xs bg-surface-alt">
            <div className="mb-2 text-warning">File changed on disk since you opened it.</div>
            <div className="flex gap-2">
              <button onClick={reloadAfterConflict} className="btn-secondary text-xs">
                Reload
              </button>
              <button onClick={() => saveFile(true)} className="btn-danger text-xs">
                Overwrite
              </button>
              <button onClick={() => setConflict(null)} className="btn-ghost text-xs">
                Dismiss
              </button>
            </div>
          </div>
        )}

        {openFile ? (
          <>
            <div className="flex items-center gap-2 mb-2 shrink-0 sticky top-0 bg-surface py-1">
              <span className="flex-1 text-xs text-text-secondary truncate">
                {openFile}
                {dirty ? ' *' : ''}
              </span>
              <button
                onClick={() => saveFile(false)}
                disabled={!dirty || saving}
                className="btn-primary text-xs min-h-10 disabled:opacity-40"
              >
                {saving ? 'Saving…' : 'Save'}
              </button>
            </div>
            <Suspense
              fallback={
                <div className="text-xs text-text-muted p-2">Loading editor…</div>
              }
            >
              <CodeEditor
                value={content}
                path={openFile}
                onChange={(v) => {
                  setContent(v)
                  setDirty(true)
                }}
                onSave={() => void saveFile(false)}
              />
            </Suspense>
          </>
        ) : (
          <div className="text-text-muted mt-10 text-center text-sm">
            Select a file to edit
          </div>
        )}
      </div>

      {menu && (
        <FileActionsMenu
          entry={menu.entry}
          x={menu.x}
          y={menu.y}
          onAction={runAction}
          onClose={() => setMenu(null)}
        />
      )}

      <input ref={fileInputRef} type="file" className="hidden" onChange={onFilePicked} />
    </div>
  )
}

function joinPath(dir: string, name: string): string {
  const trimmedDir = dir.replace(/\/+$/, '')
  const trimmedName = name.replace(/^\/+/, '')
  if (!trimmedDir) return trimmedName
  return `${trimmedDir}/${trimmedName}`
}
