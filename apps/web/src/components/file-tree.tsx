import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { List, type RowComponentProps } from 'react-window'

export interface FileEntry {
  name: string
  is_dir: boolean
  size: number
  modified: number
}

interface FileTreeProps {
  currentPath: string
  entries: FileEntry[]
  selectedPath: string | null
  onOpenDir: (name: string) => void
  onOpenFile: (name: string) => void
  onGoUp: () => void
  onContextMenu: (entry: FileEntry | null, x: number, y: number) => void
}

type Row =
  | { kind: 'parent' }
  | { kind: 'entry'; entry: FileEntry }

const ROW_HEIGHT = 30
const LONG_PRESS_MS = 500

export default function FileTree({
  currentPath,
  entries,
  selectedPath,
  onOpenDir,
  onOpenFile,
  onGoUp,
  onContextMenu,
}: FileTreeProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState(0)

  useEffect(() => {
    if (!containerRef.current) return
    const el = containerRef.current
    const ro = new ResizeObserver((entries) => {
      const h = entries[0]?.contentRect.height ?? 0
      setHeight(Math.max(0, Math.floor(h)))
    })
    ro.observe(el)
    setHeight(el.clientHeight)
    return () => ro.disconnect()
  }, [])

  const rows = useMemo<Row[]>(() => {
    const r: Row[] = []
    if (currentPath) r.push({ kind: 'parent' })
    for (const entry of entries) r.push({ kind: 'entry', entry })
    return r
  }, [currentPath, entries])

  const rowProps = useMemo(
    () => ({
      rows,
      currentPath,
      selectedPath,
      onOpenDir,
      onOpenFile,
      onGoUp,
      onContextMenu,
    }),
    [rows, currentPath, selectedPath, onOpenDir, onOpenFile, onGoUp, onContextMenu],
  )

  return (
    <div ref={containerRef} className="h-full min-h-[120px]">
      {rows.length === 0 ? (
        <div className="text-text-muted text-xs px-2 py-4">Empty directory</div>
      ) : height > 0 ? (
        <List
          rowComponent={FileRow}
          rowCount={rows.length}
          rowHeight={ROW_HEIGHT}
          rowProps={rowProps}
          overscanCount={8}
          style={{ height }}
        />
      ) : null}
    </div>
  )
}

type RowProps = {
  rows: Row[]
  currentPath: string
  selectedPath: string | null
  onOpenDir: (name: string) => void
  onOpenFile: (name: string) => void
  onGoUp: () => void
  onContextMenu: (entry: FileEntry | null, x: number, y: number) => void
}

function FileRow({ index, style, rows, currentPath, selectedPath, onOpenDir, onOpenFile, onGoUp, onContextMenu }: RowComponentProps<RowProps>) {
  const row = rows[index]
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pressOpenedMenu = useRef(false)

  const clearLongPress = useCallback(() => {
    if (pressTimer.current) {
      clearTimeout(pressTimer.current)
      pressTimer.current = null
    }
  }, [])

  if (!row) return null

  if (row.kind === 'parent') {
    return (
      <div
        style={style}
        onClick={onGoUp}
        className="cursor-pointer px-2 py-1 text-sm text-accent hover:text-accent-hover hover:bg-surface-hover rounded truncate"
      >
        ..
      </div>
    )
  }

  const entry = row.entry
  const fullPath = currentPath ? `${currentPath}/${entry.name}` : entry.name
  const isSelected = selectedPath === fullPath

  const onClick = () => {
    if (pressOpenedMenu.current) {
      pressOpenedMenu.current = false
      return
    }
    if (entry.is_dir) onOpenDir(entry.name)
    else onOpenFile(entry.name)
  }

  const onContext = (e: React.MouseEvent) => {
    e.preventDefault()
    onContextMenu(entry, e.clientX, e.clientY)
  }

  const onTouchStart = (e: React.TouchEvent) => {
    pressOpenedMenu.current = false
    const t = e.touches[0]
    if (!t) return
    const x = t.clientX
    const y = t.clientY
    pressTimer.current = setTimeout(() => {
      pressOpenedMenu.current = true
      onContextMenu(entry, x, y)
    }, LONG_PRESS_MS)
  }

  return (
    <div
      style={style}
      onClick={onClick}
      onContextMenu={onContext}
      onTouchStart={onTouchStart}
      onTouchEnd={clearLongPress}
      onTouchMove={clearLongPress}
      onTouchCancel={clearLongPress}
      className={[
        'cursor-pointer px-2 py-1 text-sm truncate hover:bg-surface-hover rounded',
        entry.is_dir ? 'text-accent font-semibold' : 'text-text-secondary',
        isSelected ? 'bg-surface-hover' : '',
      ].join(' ')}
    >
      <span className="mr-1">{entry.is_dir ? '📁' : ''}</span>
      {entry.name}
      {!entry.is_dir && (
        <span className="ml-2 text-text-muted text-xs">{fmtSize(entry.size)}</span>
      )}
    </div>
  )
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K`
  return `${(n / (1024 * 1024)).toFixed(1)}M`
}
