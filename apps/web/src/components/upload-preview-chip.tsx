// Thumbnail preview chip rendered after a successful upload. Lets the user
// catch wrong-screenshot pastes before they run the command — the dismiss
// button writes a backspace to the focused pane (caller-owned) so the
// inserted path is removed too.
//
// Image previews use `URL.createObjectURL` for zero-network thumbs; the URL
// is revoked on unmount to avoid blob-url leaks. Non-image files get a
// generic icon-only chip with the filename.

import { useEffect, useState } from 'react'
import { XIcon } from './icons'

export interface UploadPreviewChipProps {
  fileName: string
  file: File | null
  onDismiss: () => void
}

export function UploadPreviewChip({ fileName, file, onDismiss }: UploadPreviewChipProps) {
  const [thumb, setThumb] = useState<string | null>(null)

  useEffect(() => {
    if (!file || !file.type.startsWith('image/')) return
    const url = URL.createObjectURL(file)
    setThumb(url)
    return () => URL.revokeObjectURL(url)
  }, [file])

  return (
    <div
      className="pointer-events-auto flex max-w-xs items-center gap-2 rounded-md border border-border bg-surface px-2 py-1.5 shadow-sm backdrop-blur"
      role="status"
    >
      {thumb ? (
        <img
          src={thumb}
          alt=""
          className="h-9 w-9 shrink-0 rounded object-cover"
        />
      ) : (
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded bg-surface-alt text-[10px] uppercase text-text-muted">
          file
        </div>
      )}
      <span className="flex-1 truncate text-xs text-text-primary" title={fileName}>
        {fileName}
      </span>
      <button
        type="button"
        className="rounded p-0.5 text-text-muted hover:bg-surface-hover hover:text-text-primary"
        onClick={onDismiss}
        aria-label="Remove inserted path"
      >
        <XIcon size={12} />
      </button>
    </div>
  )
}
