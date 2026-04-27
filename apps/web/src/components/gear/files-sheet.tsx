import { lazy, Suspense } from 'react'
import Sheet from '../ui/sheet'

// Lazy so the gear drawer's bundle doesn't grow until the user actually
// opens the sheet — keeps initial JS budget unaffected.
const FilesPage = lazy(() => import('../../pages/files-page'))

type Props = {
  open: boolean
  onClose: () => void
}

// Compact-mode wrapper around FilesPage. The page reads hostId via useParams
// and renders <Navigate> guards if there's no active workspace; inside a
// sheet that just routes the user to /workspaces — acceptable, since opening
// Files without a workspace is a no-op anyway.
export default function FilesSheet({ open, onClose }: Props) {
  return (
    <Sheet
      open={open}
      onClose={onClose}
      side="right"
      panelClassName="sm:!w-[min(960px,90vw)]"
      ariaLabelledBy="files-sheet-title"
    >
      <div className="flex items-center justify-between px-4 py-3 border-b border-border sticky top-0 bg-surface z-10">
        <div id="files-sheet-title" className="text-text-primary font-semibold text-sm">
          Files
        </div>
        <button
          onClick={onClose}
          aria-label="Close Files"
          className="text-text-muted hover:text-text-primary text-lg leading-none w-7 h-7 flex items-center justify-center rounded hover:bg-surface-hover"
        >
          ×
        </button>
      </div>
      <Suspense fallback={<div className="p-6 text-text-muted text-sm">Loading…</div>}>
        <FilesPage />
      </Suspense>
    </Sheet>
  )
}
