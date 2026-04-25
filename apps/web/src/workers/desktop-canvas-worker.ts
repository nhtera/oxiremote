// OffscreenCanvas tile compositor — runs in a dedicated Web Worker.
// Main thread transfers canvas via init message, then streams tile messages.
// Each tile is a JPEG-encoded 128×128 region at (tileX*128, tileY*128).

let ctx: OffscreenCanvasRenderingContext2D | null = null
let tileSize = 128
// Phase 04 — persisted across resize because canvas resize resets ctx state.
let smoothingEnabled = false

interface InitMessage {
  type: 'init'
  canvas: OffscreenCanvas
  tileSize?: number
}

interface TileMessage {
  type: 'tile'
  tileX: number
  tileY: number
  jpeg: Uint8Array
  lastTile: boolean
  frameTs?: number
}

interface ResizeMessage {
  type: 'resize'
  width: number
  height: number
}

interface SmoothingMessage {
  type: 'smoothing'
  enabled: boolean
}

type WorkerMessage = InitMessage | TileMessage | ResizeMessage | SmoothingMessage

self.onmessage = async (e: MessageEvent<WorkerMessage>) => {
  const msg = e.data

  if (msg.type === 'init') {
    tileSize = msg.tileSize ?? 128
    ctx = msg.canvas.getContext('2d')
    if (ctx) ctx.imageSmoothingEnabled = smoothingEnabled
    return
  }

  if (msg.type === 'resize') {
    if (ctx) {
      ctx.canvas.width = msg.width
      ctx.canvas.height = msg.height
      // Resizing the canvas resets context state — reapply smoothing.
      ctx.imageSmoothingEnabled = smoothingEnabled
    }
    return
  }

  if (msg.type === 'smoothing') {
    smoothingEnabled = msg.enabled
    if (ctx) ctx.imageSmoothingEnabled = smoothingEnabled
    return
  }

  if (msg.type === 'tile') {
    if (!ctx) return
    const { tileX, tileY, jpeg, lastTile, frameTs } = msg

    try {
      // Cast to ArrayBuffer to satisfy strict BlobPart typing (jpeg.buffer is ArrayBufferLike)
      const blob = new Blob([jpeg.buffer as ArrayBuffer], { type: 'image/jpeg' })
      const bitmap = await createImageBitmap(blob)
      ctx.drawImage(bitmap, tileX * tileSize, tileY * tileSize)
      bitmap.close()
    } catch {
      // Malformed JPEG — skip tile silently
      return
    }

    // On last tile of frame: post FPS telemetry back to main thread
    if (lastTile) {
      self.postMessage({ type: 'frame', frameTs: frameTs ?? performance.now() })
    }
  }
}
