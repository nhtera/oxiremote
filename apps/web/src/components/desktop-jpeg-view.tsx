// JPEG-over-DataChannel desktop view — Phase 02 pipeline.
//
// Mounts the OffscreenCanvas worker (with main-thread 2D fallback) and runs
// the `useDesktopSession` hook. Pipelines incoming tile frames from the
// server onto the canvas. Pushes session state + API up to the parent so
// the shared shell (toolbar, gesture help, reconnect modal) stays DRY.

import { useCallback, useEffect, useRef } from 'react'
import {
  useDesktopSession,
  type DesktopInputEvent,
  type DesktopStatus,
  type QualityTier,
} from '../hooks/use-desktop-session'
import { useDesktopInput, type InputMode } from '../hooks/use-desktop-input'

const supportsOffscreen = typeof OffscreenCanvas !== 'undefined'

interface SessionSnapshot {
  status: DesktopStatus
  attempt: number
  screenDims?: { width: number; height: number }
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  disconnect: () => void
}

interface Props {
  hostId: string
  deviceId: string
  quality: QualityTier
  inputMode: InputMode
  monitorDefault?: { width: number; height: number }
  onSessionChange: (s: SessionSnapshot) => void
  onSessionApi: (api: SessionApi) => void
}

export default function DesktopJpegView({
  hostId,
  deviceId,
  quality,
  inputMode,
  monitorDefault,
  onSessionChange,
  onSessionApi,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const workerRef = useRef<Worker | null>(null)
  const ctx2dRef = useRef<CanvasRenderingContext2D | null>(null)
  const workerInitialized = useRef(false)

  // Tile callback — forwarded to worker or drawn on main thread.
  const onTile = useCallback((buf: ArrayBuffer) => {
    if (buf.byteLength < 5) return
    const view = new DataView(buf)
    const tileX = view.getUint16(0, false)
    const tileY = view.getUint16(2, false)

    if (supportsOffscreen && workerRef.current) {
      const jpeg = new Uint8Array(buf.slice(5))
      workerRef.current.postMessage(
        { type: 'tile', tileX, tileY, jpeg, lastTile: true, frameTs: performance.now() },
        [jpeg.buffer],
      )
    } else {
      const jpeg = new Uint8Array(buf, 5)
      const blob = new Blob([jpeg], { type: 'image/jpeg' })
      createImageBitmap(blob)
        .then((bmp) => {
          ctx2dRef.current?.drawImage(bmp, tileX * 128, tileY * 128)
          bmp.close()
        })
        .catch(() => {})
    }
  }, [])

  const { status, sendInput, setQuality, disconnect, attempt, screenDims } =
    useDesktopSession(hostId, deviceId, onTile, quality)

  // Push session state to the parent whenever it changes.
  useEffect(() => {
    onSessionChange({ status, attempt, screenDims })
  }, [status, attempt, screenDims, onSessionChange])

  // Push a stable API reference whenever the callback identities change.
  useEffect(() => {
    onSessionApi({ sendInput, setQuality, disconnect })
  }, [sendInput, setQuality, disconnect, onSessionApi])

  // Init canvas worker once — one-shot `transferControlToOffscreen` per canvas.
  useEffect(() => {
    if (workerInitialized.current || !canvasRef.current) return
    workerInitialized.current = true

    canvasRef.current.width = monitorDefault?.width ?? 1920
    canvasRef.current.height = monitorDefault?.height ?? 1080

    if (supportsOffscreen) {
      const worker = new Worker(
        new URL('../workers/desktop-canvas-worker.ts', import.meta.url),
        { type: 'module' },
      )
      workerRef.current = worker
      const offscreen = canvasRef.current.transferControlToOffscreen()
      worker.postMessage({ type: 'init', canvas: offscreen, tileSize: 128 }, [offscreen])
    } else {
      ctx2dRef.current = canvasRef.current.getContext('2d')
    }
    // See desktop-page.tsx history for why no cleanup: worker owns a
    // transferred canvas and StrictMode double-invoke would kill it.
  }, [monitorDefault])

  // Resize canvas on encoder output changes (server re-emits capabilities
  // on tier changes so the tileX*128/tileY*128 grid stays aligned).
  useEffect(() => {
    if (!screenDims) return
    const { width, height } = screenDims
    if (supportsOffscreen && workerRef.current) {
      workerRef.current.postMessage({ type: 'resize', width, height })
    } else if (canvasRef.current) {
      canvasRef.current.width = width
      canvasRef.current.height = height
    }
  }, [screenDims])

  return <CanvasWithInput canvasRef={canvasRef} mode={inputMode} sendInput={sendInput} />
}

function CanvasWithInput({
  canvasRef,
  mode,
  sendInput,
}: {
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  mode: InputMode
  sendInput: (ev: DesktopInputEvent) => void
}) {
  useDesktopInput({ canvas: canvasRef, mode, sendInput })
  return (
    <canvas
      ref={canvasRef}
      className="w-full h-full object-contain"
      style={{ display: 'block', touchAction: 'none' }}
    />
  )
}
