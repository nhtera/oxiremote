// OffscreenCanvas tile compositor — runs in a dedicated Web Worker.
// Main thread transfers canvas via init message, then streams tile messages.
// Each tile is a JPEG-encoded `tileSize`-px region at (tileX*tileSize, tileY*tileSize).
//
// Decode hot path picks the fastest API the runtime supports:
//   1. WebCodecs `ImageDecoder` (Chrome 94+, Safari 17.4+, Firefox 130+) —
//      no Blob round-trip, direct VideoFrame, explicit close() for memory.
//   2. `createImageBitmap(Blob)` fallback — works everywhere, allocates a
//      Blob per tile.

let ctx: OffscreenCanvasRenderingContext2D | null = null
let tileSize = 64
// Persisted across resize because canvas resize resets ctx state.
let smoothingEnabled = false
// `true` only after async feature detection completes and confirms the API
// is usable for `image/jpeg`. Until then, every tile takes the legacy path.
let useImageDecoder = false

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

interface ScreenshotMessage {
  type: 'screenshot'
}

interface TileSizeMessage {
  type: 'tileSize'
  tileSize: number
}

type WorkerMessage =
  | InitMessage
  | TileMessage
  | ResizeMessage
  | SmoothingMessage
  | ScreenshotMessage
  | TileSizeMessage

// Minimal local typing so we don't depend on lib.dom.d.ts shipping the
// (still partially supported) WebCodecs ImageDecoder definitions.
interface ImageDecoderInit {
  data: BufferSource
  type: string
}
interface ImageDecodeResult {
  image: VideoFrame
  complete: boolean
}
interface ImageDecoderLike {
  decode(): Promise<ImageDecodeResult>
  close(): void
}
interface ImageDecoderCtor {
  new (init: ImageDecoderInit): ImageDecoderLike
  isTypeSupported(type: string): Promise<boolean>
}

// Probe at startup so the hot path skips the feature-detection on every tile.
;(async () => {
  const ctor = (self as unknown as { ImageDecoder?: ImageDecoderCtor }).ImageDecoder
  if (!ctor) return
  try {
    const ok = await ctor.isTypeSupported('image/jpeg')
    if (ok) useImageDecoder = true
  } catch {
    // Older partial implementations throw; stay on the bitmap path.
  }
})()

async function decodeAndDraw(jpeg: Uint8Array, dx: number, dy: number) {
  if (!ctx) return
  if (useImageDecoder) {
    const ctor = (self as unknown as { ImageDecoder?: ImageDecoderCtor }).ImageDecoder!
    try {
      // Hand the decoder a fresh ArrayBuffer-backed view; the strict
      // BufferSource typing rejects the SharedArrayBuffer-tagged variant.
      const data = new Uint8Array(jpeg.buffer as ArrayBuffer, jpeg.byteOffset, jpeg.byteLength)
      const decoder = new ctor({ data, type: 'image/jpeg' })
      const { image } = await decoder.decode()
      ctx.drawImage(image as unknown as CanvasImageSource, dx, dy)
      image.close()
      decoder.close()
      return
    } catch {
      // Drop down to the bitmap path on the rare malformed-tile / unsupported
      // colour-profile case. Disabling the fast path globally would give up
      // performance on every subsequent tile, so we accept a one-tile retry.
    }
  }
  try {
    const blob = new Blob([jpeg.buffer as ArrayBuffer], { type: 'image/jpeg' })
    const bitmap = await createImageBitmap(blob)
    ctx.drawImage(bitmap, dx, dy)
    bitmap.close()
  } catch {
    // Malformed JPEG — skip tile silently
  }
}

self.onmessage = async (e: MessageEvent<WorkerMessage>) => {
  const msg = e.data

  if (msg.type === 'init') {
    tileSize = msg.tileSize ?? 64
    ctx = msg.canvas.getContext('2d')
    if (ctx) ctx.imageSmoothingEnabled = smoothingEnabled
    return
  }

  if (msg.type === 'tileSize') {
    if (msg.tileSize === 64 || msg.tileSize === 128) tileSize = msg.tileSize
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

  if (msg.type === 'screenshot') {
    if (!ctx) {
      self.postMessage({ type: 'screenshot', error: 'Worker not initialized' })
      return
    }
    try {
      const blob = await ctx.canvas.convertToBlob({ type: 'image/png' })
      self.postMessage({ type: 'screenshot', blob })
    } catch (err) {
      self.postMessage({ type: 'screenshot', error: String(err) })
    }
    return
  }

  if (msg.type === 'tile') {
    if (!ctx) return
    const { tileX, tileY, jpeg, lastTile, frameTs } = msg

    await decodeAndDraw(jpeg, tileX * tileSize, tileY * tileSize)

    // On last tile of frame: post FPS telemetry back to main thread
    if (lastTile) {
      self.postMessage({ type: 'frame', frameTs: frameTs ?? performance.now() })
    }
  }
}
