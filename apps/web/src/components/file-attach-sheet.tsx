import { useRef } from 'react'
import { useFileUpload } from '../hooks/use-file-upload'
import { ATTACHMENTS_DIR, ensureAttachmentsDir } from '../lib/ensure-attachments-dir'

// Bottom sheet (mobile) / centered modal (desktop) opened by the composer's
// paperclip button. Reuses POST /api/files/upload (multipart: dir + file)
// via the shared `useFileUpload` hook — same path used by paste/drop.
//
// Three distinct rows match the iOS share-sheet pattern (Photos / Camera /
// Files). Each row owns its own <input type="file"> so the file picker shows
// the right source UI on iOS — `accept` and `capture` only affect the picker
// when set on the actual input the user clicks.

type Props = {
  wsId: number
  dir?: string
  onPathInsert: (path: string) => void
  onClose: () => void
  /** Layout variant. `sheet` (default): bottom sheet for mobile.
   *  `modal`: centered card for desktop. */
  variant?: 'sheet' | 'modal'
}

export default function FileAttachSheet({
  wsId,
  dir = '',
  onPathInsert,
  onClose,
  variant = 'sheet',
}: Props) {
  const photoInputRef = useRef<HTMLInputElement>(null)
  const cameraInputRef = useRef<HTMLInputElement>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const { upload, reset, state, progress, error } = useFileUpload(wsId, dir)

  async function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (!file) return
    // For `.attachments/` uploads, make sure the dir exists before posting.
    // Closes the race window where the picker resolves faster than the
    // workspace-page's fire-and-forget mkdir on a slow tunnel.
    if (dir === ATTACHMENTS_DIR) {
      await ensureAttachmentsDir(wsId)
    }
    upload(file)
      .then((res) => {
        onPathInsert(res.path)
        onClose()
      })
      .catch(() => {
        // Hook already populated `error`; UI renders below.
      })
  }

  // Modal variant centers a card; sheet variant pins to bottom + adds a grab
  // handle and safe-area padding. Picker rows + auth/upload logic are identical.
  const isModal = variant === 'modal'
  const outerCls = isModal
    ? 'fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4'
    : 'fixed inset-0 z-50 flex flex-col justify-end bg-black/60'
  const innerCls = isModal
    ? 'w-full max-w-md bg-surface border border-border rounded-2xl p-4 shadow-xl'
    : 'w-full bg-surface border-t border-border rounded-t-xl p-4 pb-8'
  const innerStyle: React.CSSProperties | undefined = isModal
    ? undefined
    : { paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 1.5rem)' }

  return (
    <div role="dialog" aria-modal="true" className={outerCls} onClick={onClose}>
      <div className={innerCls} onClick={(e) => e.stopPropagation()} style={innerStyle}>
        {!isModal && <div className="mx-auto mb-3 h-1 w-10 rounded-full bg-border" />}

        <div className="text-sm font-semibold text-text-primary mb-3">Attach file</div>

        {state === 'idle' && (
          <div className="flex flex-col gap-1">
            <SheetOption
              icon={<PhotoIcon />}
              label="Photo Library"
              desc="Pick an image or video from your library"
              onClick={() => photoInputRef.current?.click()}
            />
            <SheetOption
              icon={<CameraIcon />}
              label="Take Photo or Video"
              desc="Capture with the camera"
              onClick={() => cameraInputRef.current?.click()}
            />
            <SheetOption
              icon={<FolderIcon />}
              label="Choose File"
              desc="Upload any file from your device"
              onClick={() => fileInputRef.current?.click()}
            />
            <button
              onClick={onClose}
              className="mt-2 px-4 py-2.5 text-sm text-text-secondary border border-border rounded-md hover:bg-surface-hover"
            >
              Cancel
            </button>
          </div>
        )}

        {state === 'uploading' && (
          <div className="flex flex-col gap-2 py-3">
            <div className="text-xs text-text-muted">Uploading… {progress}%</div>
            <div className="h-1.5 w-full rounded-full bg-surface-alt overflow-hidden">
              <div
                className="h-full bg-accent transition-all"
                style={{ width: `${progress}%` }}
              />
            </div>
          </div>
        )}

        {state === 'error' && (
          <div className="flex flex-col gap-2 py-3">
            <div className="text-xs text-danger bg-danger/10 border border-danger/30 rounded px-2 py-1">
              {error?.msg ?? 'Upload failed'}
            </div>
            <div className="flex gap-2">
              <button
                onClick={reset}
                className="btn-secondary flex-1 text-sm py-2"
              >
                Retry
              </button>
              <button
                onClick={onClose}
                className="px-4 py-2 text-sm text-text-secondary border border-border rounded-md"
              >
                Close
              </button>
            </div>
          </div>
        )}

        <input
          ref={photoInputRef}
          type="file"
          accept="image/*,video/*"
          className="hidden"
          onChange={handleFileChange}
        />
        <input
          ref={cameraInputRef}
          type="file"
          accept="image/*,video/*"
          capture="environment"
          className="hidden"
          onChange={handleFileChange}
        />
        <input
          ref={fileInputRef}
          type="file"
          className="hidden"
          onChange={handleFileChange}
        />
      </div>
    </div>
  )
}

function SheetOption({
  icon,
  label,
  desc,
  onClick,
}: {
  icon: React.ReactNode
  label: string
  desc: string
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-3 text-left px-4 py-3 rounded-md border border-border hover:bg-surface-hover"
    >
      <span className="shrink-0 w-9 h-9 rounded-md bg-surface flex items-center justify-center text-accent">
        {icon}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium text-text-primary">{label}</span>
        <span className="block text-xs text-text-muted">{desc}</span>
      </span>
    </button>
  )
}

function PhotoIcon() {
  return (
    <svg viewBox="0 0 24 24" width={18} height={18} fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <circle cx="9" cy="11" r="2" />
      <path d="M3 17l5-4 4 3 4-5 5 6" />
    </svg>
  )
}

function CameraIcon() {
  return (
    <svg viewBox="0 0 24 24" width={18} height={18} fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 7h3l2-2h6l2 2h3a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1z" />
      <circle cx="12" cy="13" r="3.5" />
    </svg>
  )
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 24 24" width={18} height={18} fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </svg>
  )
}
