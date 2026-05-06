// Reusable workspace-file upload via XHR. Picker, paste, drag-drop, and the
// desktop attach button all consume this — auth (Bearer + CSRF), CORS
// (discovery vs embedded), progress, and cancellation behave the same.
//
// Mirrors the agent-side cap in `agent/src/files_upload.rs` (MAX_UPLOAD_BYTES
// = 100 * 1024 * 1024). Rejecting oversize early avoids a wasted multipart
// stream + 413 round-trip.
//
// Two surfaces:
//   - `useFileUpload(wsId, dir)` — stateful hook for the picker sheet (one
//     active upload, exposes progress + state machine + cancel + reset).
//   - `uploadFile({ wsId, dir, file, onProgress, signal })` — bare imperative
//     call for paste/drop where the caller manages its own state per drop.

import { useCallback, useRef, useState } from 'react'
import { loadApiKey, loadTunnelBase } from '../lib/api-client'
import { isDiscoveryMode } from '../lib/discovery-client'

export const MAX_UPLOAD_BYTES = 100 * 1024 * 1024

export type UploadErrorKind =
  | 'too_large'
  | 'conflict'
  | 'network'
  | 'cancelled'
  | 'http'
  | 'invalid_response'

export interface UploadError {
  kind: UploadErrorKind
  msg: string
  status?: number
}

export interface UploadResult {
  path: string
  size: number
}

export type UploadState = 'idle' | 'uploading' | 'error' | 'success'

export interface UseFileUpload {
  upload: (file: File, opts?: { id?: string }) => Promise<UploadResult>
  cancel: () => void
  reset: () => void
  state: UploadState
  progress: number
  error: UploadError | null
  /** Last `id` passed to upload(). Useful for callers that track multiple
   *  in-flight uploads externally (chip stack). */
  lastId: string | null
}

const CSRF_COOKIE = 'oxi_csrf'

function readCsrfCookie(): string | null {
  if (typeof document === 'undefined') return null
  for (const part of document.cookie.split(';')) {
    const trimmed = part.trim()
    if (trimmed.startsWith(CSRF_COOKIE + '=')) {
      return trimmed.slice(CSRF_COOKIE.length + 1)
    }
  }
  return null
}

export interface UploadFileArgs {
  wsId: number
  dir?: string
  file: File
  onProgress?: (pct: number) => void
  /** Pass an AbortController.signal to allow cancellation. */
  signal?: AbortSignal
}

/** Bare imperative upload — no React state. Resolves with the workspace path
 *  + size; rejects with `UploadError`. Used by paste/drop where the caller
 *  manages its own UI state per upload. The stateful `useFileUpload` below
 *  wraps this for the picker. */
export function uploadFile({
  wsId,
  dir = '',
  file,
  onProgress,
  signal,
}: UploadFileArgs): Promise<UploadResult> {
  if (file.size > MAX_UPLOAD_BYTES) {
    return Promise.reject<UploadResult>({
      kind: 'too_large',
      msg: `File exceeds 100 MB limit (${(file.size / 1024 / 1024).toFixed(1)} MB)`,
    } satisfies UploadError)
  }

  return new Promise<UploadResult>((resolve, reject) => {
    const form = new FormData()
    form.append('dir', dir)
    form.append('file', file, file.name)

    // `on_conflict=suffix` makes the agent walk `name (1).ext`, `name (2).ext`, ...
    // when the destination exists. Chat-style attach flows (paste/drop/picker/
    // desktop modal) all share this hook, so they all opt in. The files-page
    // upload uses a bare `fetch` and stays on the default `fail` semantics where
    // a conflict surfaces a real error to the user.
    const path = `/api/files/upload?ws_id=${wsId}&on_conflict=suffix`
    const tunnelBase = isDiscoveryMode() ? loadTunnelBase() : null
    const crossOrigin = tunnelBase !== null
    const url = crossOrigin ? `${tunnelBase}${path}` : path

    const xhr = new XMLHttpRequest()
    xhr.open('POST', url)
    xhr.withCredentials = !crossOrigin
    const apiKey = loadApiKey()
    if (apiKey) xhr.setRequestHeader('Authorization', `Bearer ${apiKey}`)
    if (!crossOrigin) {
      const csrf = readCsrfCookie()
      if (csrf) xhr.setRequestHeader('X-OXI-CSRF', csrf)
    }
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && onProgress) {
        onProgress(Math.round((e.loaded / e.total) * 100))
      }
    }
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as UploadResult)
        } catch {
          reject({ kind: 'invalid_response', msg: 'Invalid response', status: xhr.status } satisfies UploadError)
        }
      } else if (xhr.status === 409) {
        reject({ kind: 'conflict', msg: xhr.responseText || 'Destination exists', status: 409 } satisfies UploadError)
      } else if (xhr.status === 413) {
        reject({ kind: 'too_large', msg: xhr.responseText || 'Too large', status: 413 } satisfies UploadError)
      } else {
        reject({ kind: 'http', msg: xhr.responseText || `Upload failed (${xhr.status})`, status: xhr.status } satisfies UploadError)
      }
    }
    xhr.onerror = () => reject({ kind: 'network', msg: 'Network error' } satisfies UploadError)
    xhr.onabort = () => reject({ kind: 'cancelled', msg: 'Upload cancelled' } satisfies UploadError)
    if (signal) {
      if (signal.aborted) {
        xhr.abort()
        return
      }
      signal.addEventListener('abort', () => xhr.abort(), { once: true })
    }
    xhr.send(form)
  })
}

export function useFileUpload(wsId: number, dir: string = ''): UseFileUpload {
  const [state, setState] = useState<UploadState>('idle')
  const [progress, setProgress] = useState(0)
  const [error, setError] = useState<UploadError | null>(null)
  const [lastId, setLastId] = useState<string | null>(null)
  const abortRef = useRef<AbortController | null>(null)

  const reset = useCallback(() => {
    abortRef.current = null
    setState('idle')
    setProgress(0)
    setError(null)
  }, [])

  const cancel = useCallback(() => {
    abortRef.current?.abort()
  }, [])

  const upload = useCallback(
    (file: File, opts?: { id?: string }): Promise<UploadResult> => {
      const ctrl = new AbortController()
      abortRef.current = ctrl
      setState('uploading')
      setProgress(0)
      setError(null)
      if (opts?.id) setLastId(opts.id)

      return uploadFile({
        wsId,
        dir,
        file,
        signal: ctrl.signal,
        onProgress: (pct) => setProgress(pct),
      })
        .then((res) => {
          setState('success')
          setProgress(100)
          return res
        })
        .catch((err: UploadError) => {
          if (err.kind === 'cancelled') {
            setState('idle')
            setError(null)
          } else {
            setState('error')
            setError(err)
          }
          throw err
        })
    },
    [wsId, dir],
  )

  return { upload, cancel, reset, state, progress, error, lastId }
}
