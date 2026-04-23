import { useEffect, useRef } from 'react'
import type { FileEntry } from './file-tree'

export type FileAction =
  | 'new-file'
  | 'new-folder'
  | 'rename'
  | 'delete'
  | 'upload'
  | 'download'

interface Props {
  entry: FileEntry | null
  x: number
  y: number
  onAction: (action: FileAction) => void
  onClose: () => void
}

export default function FileActionsMenu({ entry, x, y, onAction, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    function onDocDown(e: MouseEvent | TouchEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('mousedown', onDocDown)
    document.addEventListener('touchstart', onDocDown, { passive: true })
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDocDown)
      document.removeEventListener('touchstart', onDocDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  // Clamp to viewport so we don't overflow on mobile edges.
  const vw = typeof window === 'undefined' ? 0 : window.innerWidth
  const vh = typeof window === 'undefined' ? 0 : window.innerHeight
  const left = Math.min(x, vw - 200)
  const top = Math.min(y, vh - 260)

  const items: { id: FileAction; label: string; show: boolean; danger?: boolean }[] = [
    { id: 'new-file', label: 'New file', show: true },
    { id: 'new-folder', label: 'New folder', show: true },
    { id: 'upload', label: 'Upload here', show: !entry || entry.is_dir },
    { id: 'rename', label: 'Rename', show: !!entry },
    { id: 'download', label: 'Download', show: !!entry && !entry.is_dir },
    { id: 'delete', label: 'Delete', show: !!entry, danger: true },
  ]

  return (
    <div
      ref={ref}
      style={{ position: 'fixed', left, top, zIndex: 60 }}
      className="min-w-[180px] bg-surface-alt border border-border rounded-md shadow-lg py-1 text-sm"
    >
      {items
        .filter((i) => i.show)
        .map((i) => (
          <button
            key={i.id}
            onClick={() => onAction(i.id)}
            className={`w-full text-left px-3 py-2 min-h-10 hover:bg-surface-hover ${
              i.danger ? 'text-danger' : 'text-text-primary'
            }`}
          >
            {i.label}
          </button>
        ))}
    </div>
  )
}
