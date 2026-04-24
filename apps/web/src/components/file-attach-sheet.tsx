import { useRef, useState } from 'react'

// Bottom sheet opened by the composer's paperclip button.
// Reuses POST /api/files/upload (multipart: dir + file) — the same endpoint
// the FilesPage upload flow uses. On success, inserts `"<workspace-relative path>"`
// into the composer via `onPathInsert`.

type Props = {
  wsId: number
  dir?: string
  onPathInsert: (path: string) => void
  onClose: () => void
}

type UploadState =
  | { kind: 'idle' }
  | { kind: 'uploading'; pct: number }
  | { kind: 'error'; msg: string }

export default function FileAttachSheet({ wsId, dir = '', onPathInsert, onClose }: Props) {
  const fileInputRef = useRef<HTMLInputElement>(null)
  const cameraInputRef = useRef<HTMLInputElement>(null)
  const [state, setState] = useState<UploadState>({ kind: 'idle' })

  const cameraSupported =
    typeof navigator !== 'undefined' && !!navigator.mediaDevices

  function uploadFile(file: File) {
    setState({ kind: 'uploading', pct: 0 })

    const form = new FormData()
    form.append('dir', dir)
    form.append('file', file, file.name)

    // Use XHR for progress events; `fetch` doesn't expose upload progress.
    const xhr = new XMLHttpRequest()
    xhr.open('POST', `/api/files/upload?ws_id=${wsId}`)
    xhr.withCredentials = true
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) {
        setState({ kind: 'uploading', pct: Math.round((e.loaded / e.total) * 100) })
      }
    }
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          const data = JSON.parse(xhr.responseText) as { path: string; size: number }
          onPathInsert(data.path)
          onClose()
        } catch {
          setState({ kind: 'error', msg: 'Upload succeeded but response was invalid' })
        }
      } else {
        setState({ kind: 'error', msg: xhr.responseText || `Upload failed (${xhr.status})` })
      }
    }
    xhr.onerror = () => setState({ kind: 'error', msg: 'Network error' })
    xhr.send(form)
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (file) uploadFile(file)
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex flex-col justify-end bg-black/60"
      onClick={onClose}
    >
      <div
        className="w-full bg-surface border-t border-border rounded-t-xl p-4 pb-8"
        onClick={(e) => e.stopPropagation()}
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 1.5rem)' }}
      >
        <div className="mx-auto mb-3 h-1 w-10 rounded-full bg-border" />

        <div className="text-sm font-semibold text-text-primary mb-3">Attach file</div>

        {state.kind === 'idle' && (
          <div className="flex flex-col gap-1">
            <SheetOption
              label="Choose File"
              desc="Upload a file from your device"
              onClick={() => fileInputRef.current?.click()}
            />
            {cameraSupported && (
              <SheetOption
                label="Take Photo"
                desc="Capture with the camera"
                onClick={() => cameraInputRef.current?.click()}
              />
            )}
            <button
              onClick={onClose}
              className="mt-2 px-4 py-2.5 text-sm text-text-secondary border border-border rounded-md hover:bg-surface-hover"
            >
              Cancel
            </button>
          </div>
        )}

        {state.kind === 'uploading' && (
          <div className="flex flex-col gap-2 py-3">
            <div className="text-xs text-text-muted">Uploading… {state.pct}%</div>
            <div className="h-1.5 w-full rounded-full bg-surface-alt overflow-hidden">
              <div
                className="h-full bg-accent transition-all"
                style={{ width: `${state.pct}%` }}
              />
            </div>
          </div>
        )}

        {state.kind === 'error' && (
          <div className="flex flex-col gap-2 py-3">
            <div className="text-xs text-danger bg-danger/10 border border-danger/30 rounded px-2 py-1">
              {state.msg}
            </div>
            <div className="flex gap-2">
              <button
                onClick={() => setState({ kind: 'idle' })}
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
          ref={fileInputRef}
          type="file"
          className="hidden"
          onChange={handleFileChange}
        />
        <input
          ref={cameraInputRef}
          type="file"
          accept="image/*"
          capture="environment"
          className="hidden"
          onChange={handleFileChange}
        />
      </div>
    </div>
  )
}

function SheetOption({
  label,
  desc,
  onClick,
}: {
  label: string
  desc: string
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className="text-left px-4 py-3 rounded-md border border-border hover:bg-surface-hover"
    >
      <div className="text-sm font-medium text-text-primary">{label}</div>
      <div className="text-xs text-text-muted">{desc}</div>
    </button>
  )
}
