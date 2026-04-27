import { lazy, Suspense } from 'react'
import Sheet from '../ui/sheet'

const PreviewPage = lazy(() => import('../../pages/preview-page'))

type Props = {
  open: boolean
  onClose: () => void
}

// Compact-mode wrapper around PreviewPage. Self-contained — no route params.
export default function PreviewSheet({ open, onClose }: Props) {
  return (
    <Sheet
      open={open}
      onClose={onClose}
      side="right"
      panelClassName="sm:!w-[min(720px,80vw)]"
      ariaLabelledBy="preview-sheet-title"
    >
      <div className="flex items-center justify-between px-4 py-3 border-b border-border sticky top-0 bg-surface z-10">
        <div id="preview-sheet-title" className="text-text-primary font-semibold text-sm">
          Sites (Preview)
        </div>
        <button
          onClick={onClose}
          aria-label="Close preview"
          className="text-text-muted hover:text-text-primary text-lg leading-none w-7 h-7 flex items-center justify-center rounded hover:bg-surface-hover"
        >
          ×
        </button>
      </div>
      <Suspense fallback={<div className="p-6 text-text-muted text-sm">Loading…</div>}>
        <PreviewPage />
      </Suspense>
    </Sheet>
  )
}
