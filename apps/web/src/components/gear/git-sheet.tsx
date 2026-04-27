import { lazy, Suspense } from 'react'
import Sheet from '../ui/sheet'

// Lazy so the gear drawer's bundle doesn't grow until the user actually
// opens the sheet — keeps initial JS budget unaffected.
const GitPage = lazy(() => import('../../pages/git-page'))

type Props = {
  open: boolean
  onClose: () => void
}

// Compact-mode wrapper around GitPage. The page is self-contained (no route
// params, no Navigate redirects) so we can mount it directly inside a Sheet
// without extracting an internal content component.
export default function GitSheet({ open, onClose }: Props) {
  return (
    <Sheet
      open={open}
      onClose={onClose}
      side="right"
      panelClassName="sm:!w-[min(720px,80vw)]"
      ariaLabelledBy="git-sheet-title"
    >
      <div className="flex items-center justify-between px-4 py-3 border-b border-border sticky top-0 bg-surface z-10">
        <div id="git-sheet-title" className="text-text-primary font-semibold text-sm">
          Git
        </div>
        <button
          onClick={onClose}
          aria-label="Close Git"
          className="text-text-muted hover:text-text-primary text-lg leading-none w-7 h-7 flex items-center justify-center rounded hover:bg-surface-hover"
        >
          ×
        </button>
      </div>
      <Suspense fallback={<div className="p-6 text-text-muted text-sm">Loading…</div>}>
        <GitPage />
      </Suspense>
    </Sheet>
  )
}
