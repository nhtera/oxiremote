// Client-side screenshot capture for the remote desktop view. The image is
// composed entirely in the browser; nothing leaves the device. The PNG is
// triggered as a download so the user gets it in their Photos / Downloads.
//
// Inputs:
//   - HTMLCanvasElement when its bitmap lives on the main thread (H.264 view
//     blits the video frame onto the canvas, so toBlob() works).
//   - HTMLVideoElement when we need to grab the current frame from a media
//     stream (some H.264 paths) — drawn onto a fresh canvas first.
//   - { worker } message handle when the JPEG view's canvas was transferred
//     to a worker via transferControlToOffscreen — toBlob() can't read the
//     transferred bitmap, so the worker has to convertToBlob and post back.

interface DownloadOpts {
  /** Slug used in the download filename (host label). */
  label: string
}

function buildFilename(label: string): string {
  const safeLabel = label.replace(/[^a-zA-Z0-9_-]+/g, '-').slice(0, 32) || 'host'
  const ts = new Date().toISOString().replace(/[:.]/g, '-')
  return `oxiremote-${safeLabel}-${ts}.png`
}

function triggerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  // Revoke after the click loop runs the download.
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export async function captureCanvas(
  canvas: HTMLCanvasElement,
  opts: DownloadOpts,
): Promise<void> {
  const blob: Blob = await new Promise((resolve, reject) => {
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('canvas.toBlob returned null'))), 'image/png')
  })
  triggerDownload(blob, buildFilename(opts.label))
}

export async function captureVideo(
  video: HTMLVideoElement,
  opts: DownloadOpts,
): Promise<void> {
  const w = video.videoWidth
  const h = video.videoHeight
  if (!w || !h) throw new Error('Video frame not ready')
  const off =
    typeof OffscreenCanvas !== 'undefined'
      ? new OffscreenCanvas(w, h)
      : null
  let blob: Blob
  if (off) {
    const ctx = off.getContext('2d')
    if (!ctx) throw new Error('No 2D context on OffscreenCanvas')
    ctx.drawImage(video, 0, 0)
    blob = await off.convertToBlob({ type: 'image/png' })
  } else {
    const c = document.createElement('canvas')
    c.width = w
    c.height = h
    const ctx = c.getContext('2d')
    if (!ctx) throw new Error('No 2D context')
    ctx.drawImage(video, 0, 0)
    blob = await new Promise((resolve, reject) => {
      c.toBlob((b) => (b ? resolve(b) : reject(new Error('toBlob null'))), 'image/png')
    })
  }
  triggerDownload(blob, buildFilename(opts.label))
}

// Worker-side screenshot: post a message and wait for the worker to send
// back the blob. The JPEG canvas is transferred via transferControlToOffscreen
// so the main thread can't read its pixels — only the worker can.
export async function captureViaWorker(
  worker: Worker,
  opts: DownloadOpts,
  timeoutMs = 5000,
): Promise<void> {
  const blob = await new Promise<Blob>((resolve, reject) => {
    const onMsg = (e: MessageEvent) => {
      if (e.data?.type !== 'screenshot') return
      worker.removeEventListener('message', onMsg)
      window.clearTimeout(timer)
      if (e.data.error) reject(new Error(String(e.data.error)))
      else resolve(e.data.blob as Blob)
    }
    const timer = window.setTimeout(() => {
      worker.removeEventListener('message', onMsg)
      reject(new Error('Screenshot timed out'))
    }, timeoutMs)
    worker.addEventListener('message', onMsg)
    worker.postMessage({ type: 'screenshot' })
  })
  triggerDownload(blob, buildFilename(opts.label))
}
