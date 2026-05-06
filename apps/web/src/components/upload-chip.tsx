// Upload-progress chip rendered absolute at the focused pane's top-right.
// One chip per in-flight upload; multiple stack via the parent's flex column.
//
// Two visual states:
//   - uploading: shrinking progress bar + cancel ✕
//   - error:    red tint + retry / dismiss
// Success is observed by the parent (workspace-page), which drops the chip
// and pushes a preview chip — so this component never renders "success".
//
// Pointer-events on the chip pass through to xterm via `pointer-events-auto`
// only on the chip itself; the wrapping stack uses `pointer-events-none` so
// clicks elsewhere on the pane still focus the terminal.

import { XIcon } from './icons'
import type { UploadError } from '../hooks/use-file-upload'

export interface UploadChipProps {
  fileName: string
  pct: number
  state: 'uploading' | 'error'
  error?: UploadError | null
  onCancel?: () => void
  onRetry?: () => void
  onDismiss?: () => void
}

export function UploadChip({
  fileName,
  pct,
  state,
  error,
  onCancel,
  onRetry,
  onDismiss,
}: UploadChipProps) {
  const isError = state === 'error'
  const tone = isError
    ? 'border-danger/40 bg-danger/10'
    : 'border-border bg-surface'

  return (
    <div
      className={`pointer-events-auto flex w-56 flex-col gap-1 rounded-md border ${tone} px-2.5 py-1.5 shadow-sm backdrop-blur`}
    >
      {/* sr-only live region announces only state transitions, not progress %. */}
      <span className="sr-only" aria-live="polite">
        {isError ? `${fileName} upload failed${error?.msg ? `: ${error.msg}` : ''}` : ''}
      </span>
      <div className="flex items-center gap-2">
        <span className="flex-1 truncate text-xs text-text-primary" title={fileName}>
          {fileName}
        </span>
        <span className="text-[10px] tabular-nums text-text-muted">
          {isError ? 'failed' : `${pct}%`}
        </span>
        <button
          type="button"
          className="rounded p-0.5 text-text-muted hover:bg-surface-hover hover:text-text-primary"
          onClick={isError ? onDismiss : onCancel}
          aria-label={isError ? 'Dismiss' : 'Cancel upload'}
        >
          <XIcon size={12} />
        </button>
      </div>
      {isError ? (
        <div className="flex items-center justify-between gap-2">
          <span className="truncate text-[11px] text-danger" title={error?.msg}>
            {error?.msg ?? 'Upload failed'}
          </span>
          {onRetry && (
            <button
              type="button"
              className="text-[11px] text-accent hover:underline"
              onClick={onRetry}
            >
              Retry
            </button>
          )}
        </div>
      ) : (
        <div className="h-1 w-full overflow-hidden rounded-full bg-surface-alt">
          <div
            className="h-full bg-accent transition-all"
            style={{ width: `${pct}%` }}
          />
        </div>
      )}
    </div>
  )
}
