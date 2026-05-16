// JPEG-over-DataChannel desktop view.
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
import { useDesktopInput, type InputMode, type GestureMode } from '../hooks/use-desktop-input'
import { useCanvasGestures } from '../hooks/use-canvas-gestures'
import { captureCanvas, captureViaWorker } from '../lib/desktop-screenshot'
import DesktopRectOverlay, { type DesktopRectOverlayHandle } from './desktop-rect-overlay'
import DesktopCursorOverlay from './desktop-cursor-overlay'
import DesktopCursorTrackOverlay from './desktop-cursor-track-overlay'

const supportsOffscreen = typeof OffscreenCanvas !== 'undefined'

interface SessionSnapshot {
  status: DesktopStatus
  attempt: number
  screenDims?: { width: number; height: number }
  pipelineInfo?: import('../hooks/use-desktop-session').DesktopPipelineInfo
  hostLockState?: import('../hooks/use-desktop-session').HostLockState
  accessibilityMissing?: boolean
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  disconnect: () => void
  screenshot?: () => Promise<void>
}

/** Imperative handle exposed by the canvas's pinch/pan gesture stack. The
 *  shell consumes it via the Fit menu item to drop user zoom back to 1×. */
export interface DesktopGestureApi {
  resetZoom: () => void
}

interface Props {
  hostId: string
  deviceId: string
  quality: QualityTier
  inputMode: InputMode
  gestureMode?: GestureMode
  hidpi: boolean
  smoothScaling: boolean
  monitorDefault?: { width: number; height: number }
  /** Filename slug (host label) for the downloaded screenshot. */
  hostLabel: string
  onSessionChange: (s: SessionSnapshot) => void
  onSessionApi: (api: SessionApi) => void
  /** Pinch-zoom user scale (1 = unzoomed). Forwarded to the top strip's
   *  zoom % indicator so it reflects real on-screen scale. */
  onZoomChange?: (scale: number) => void
  /** Receives a stable handle for resetting zoom from outside the canvas. */
  onGestureApi?: (api: DesktopGestureApi) => void
  /** Bottom-anchor the painted image when the soft keyboard is open so it
   *  rides up with the keyboard (RVNC-style). Centred otherwise. */
  bottomAnchor?: boolean
  /** User-driven pipeline override. Sent as `?force_pipeline=` on the WS
   *  upgrade so the agent stays on this transport for the session. */
  forcePipeline?: 'h264' | 'jpeg' | 'auto'
}

export default function DesktopJpegView({
  hostId,
  deviceId,
  quality,
  inputMode,
  gestureMode = 'pointer',
  hidpi,
  smoothScaling,
  monitorDefault,
  hostLabel,
  onSessionChange,
  onSessionApi,
  onZoomChange,
  onGestureApi,
  bottomAnchor = false,
  forcePipeline,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const workerRef = useRef<Worker | null>(null)
  const ctx2dRef = useRef<CanvasRenderingContext2D | null>(null)
  const workerInitialized = useRef(false)
  // Live tile size for the main-thread fallback path. Worker has its own
  // copy via `init` / `tileSize` messages.
  const tileSizeRef = useRef<number>(64)

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
      const ts = tileSizeRef.current
      createImageBitmap(blob)
        .then((bmp) => {
          ctx2dRef.current?.drawImage(bmp, tileX * ts, tileY * ts)
          bmp.close()
        })
        .catch(() => {})
    }
  }, [])

  const {
    status,
    sendInput,
    setQuality,
    setSettings,
    disconnect,
    attempt,
    screenDims,
    tileSize,
    pipelineInfo,
    hostLockState,
    accessibilityMissing,
    cursorSnapshot,
  } = useDesktopSession(hostId, deviceId, onTile, quality, hidpi, forcePipeline)

  // Push session state to the parent whenever it changes.
  useEffect(() => {
    onSessionChange({
      status,
      attempt,
      screenDims,
      pipelineInfo,
      hostLockState,
      accessibilityMissing,
    })
  }, [status, attempt, screenDims, pipelineInfo, hostLockState, accessibilityMissing, onSessionChange])

  // Stable screenshot callback — needs access to the worker (offscreen path)
  // OR the main-thread canvas (fallback path). Defined via callback so the
  // caller can invoke it imperatively from the top strip.
  const screenshot = useCallback(async () => {
    if (workerRef.current) {
      await captureViaWorker(workerRef.current, { label: hostLabel })
    } else if (canvasRef.current) {
      await captureCanvas(canvasRef.current, { label: hostLabel })
    }
  }, [hostLabel])

  // Push a stable API reference whenever the callback identities change.
  useEffect(() => {
    onSessionApi({ sendInput, setQuality, setSettings, disconnect, screenshot })
  }, [sendInput, setQuality, setSettings, disconnect, screenshot, onSessionApi])

  // Apply smoothScaling: worker postMessage in offscreen mode, ctx flag in
  // main-thread fallback. Re-applied on every smoothScaling change AND on
  // resize (worker handles its own re-apply on resize internally).
  useEffect(() => {
    if (supportsOffscreen && workerRef.current) {
      workerRef.current.postMessage({ type: 'smoothing', enabled: smoothScaling })
    } else if (ctx2dRef.current) {
      ctx2dRef.current.imageSmoothingEnabled = smoothScaling
    }
  }, [smoothScaling])

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
      // Default to 64 — agent's current TILE_SIZE. Server re-emits the
      // authoritative value in every `capabilities` message and we forward
      // it on the dedicated effect below, but seeding the right value keeps
      // the very first tile painted at the right grid offset.
      worker.postMessage({ type: 'init', canvas: offscreen, tileSize: 64 }, [offscreen])
    } else {
      ctx2dRef.current = canvasRef.current.getContext('2d')
    }
    // See desktop-page.tsx history for why no cleanup: worker owns a
    // transferred canvas and StrictMode double-invoke would kill it.
  }, [monitorDefault])

  // Forward authoritative tile size to the worker on every capabilities
  // change (including the JPEG↔H.264 mode flip if the server ever does that
  // mid-session). Cheap enough to send unconditionally.
  useEffect(() => {
    if (typeof tileSize !== 'number') return
    tileSizeRef.current = tileSize
    if (supportsOffscreen && workerRef.current) {
      workerRef.current.postMessage({ type: 'tileSize', tileSize })
    }
  }, [tileSize])

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

  return (
    <CanvasWithInput
      canvasRef={canvasRef}
      mode={inputMode}
      gestureMode={gestureMode}
      sendInput={sendInput}
      onZoomChange={onZoomChange}
      onGestureApi={onGestureApi}
      bottomAnchor={bottomAnchor}
      screenDims={screenDims}
      cursorSnapshot={cursorSnapshot}
    />
  )
}

function CanvasWithInput({
  canvasRef,
  mode,
  gestureMode,
  sendInput,
  onZoomChange,
  onGestureApi,
  bottomAnchor,
  screenDims,
  cursorSnapshot,
}: {
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  mode: InputMode
  gestureMode: GestureMode
  sendInput: (ev: DesktopInputEvent) => void
  onZoomChange?: (scale: number) => void
  onGestureApi?: (api: DesktopGestureApi) => void
  bottomAnchor: boolean
  screenDims?: { width: number; height: number }
  cursorSnapshot: import('../lib/desktop-cursor-track').CursorSnapshot | null
}) {
  const overlayRef = useRef<DesktopRectOverlayHandle>(null)
  const layerRef = useRef<HTMLDivElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  // Stable forwarder so useDesktopInput's effect deps don't churn — the
  // imperative-handle ref isn't populated on the first render, but the
  // forwarder always reads the latest pointer.
  const overlayForwarder = useRef({
    show(start: { x: number; y: number }, end: { x: number; y: number }) {
      overlayRef.current?.show(start, end)
    },
    hide() {
      overlayRef.current?.hide()
    },
  }).current
  useDesktopInput({
    canvas: canvasRef,
    mode,
    sendInput,
    gestureMode,
    rectOverlay: overlayForwarder,
  })
  // Pointer-Events gesture stack owns 1-finger pan, pinch-zoom, and the
  // trackpad-mode virtual cursor. The sprite renders only in trackpad mode;
  // touch mode taps directly without a cursor.
  const { cursor, resetZoom, fitToViewport } = useCanvasGestures({
    target: canvasRef,
    layer: layerRef,
    viewport: viewportRef,
    mode,
    sendInput,
    onZoomChange,
    disabled: gestureMode === 'rect',
  })
  // Hand the reset-zoom handle up. Stable across renders because resetZoom
  // is memoized in the hook; running this on each render is cheap.
  useEffect(() => {
    onGestureApi?.({ resetZoom })
  }, [onGestureApi, resetZoom])
  // Smart auto-fit: kicks in once the first valid stream-dim arrives so the
  // host fills a sensible portion of the wrap. One-shot per `resetZoom` —
  // tap "Fit" in the menu to undo and re-arm.
  useEffect(() => {
    if (!screenDims) return
    fitToViewport()
  }, [screenDims, fitToViewport])
  return (
    <div
      ref={viewportRef}
      className="w-full h-full relative overflow-hidden"
      style={{ touchAction: 'none' }}
    >
      <div ref={layerRef} className="w-full h-full" style={{ willChange: 'transform' }}>
        <canvas
          ref={canvasRef}
          // Focusable so clicking the canvas takes the activeElement
          // away from any previously-focused button/input — without this
          // the global keyboard-capture hook stays disabled (it skips
          // BUTTON/INPUT focus). focus() is called from onPointerDown
          // so a single tap reclaims the keyboard for the remote.
          tabIndex={0}
          onPointerDown={(e) => e.currentTarget.focus({ preventScroll: true })}
          // Anchor the painted desktop to the bottom of the wrap ONLY
          // while the soft keyboard is open — that way the image rides
          // up with the keyboard (RVNC pattern). When the keyboard is
          // closed we fall back to centred so the desktop sits balanced
          // in the available space (no oddly-empty top half).
          className={`w-full h-full object-contain outline-none ${bottomAnchor ? 'object-bottom' : ''}`}
          // Hide the local browser cursor only when the cursor sideband
          // (`DesktopCursorTrackOverlay`) is actively rendering the host
          // pointer. Without this the user sees two stacked arrows on
          // Windows hosts. Gated so macOS sessions (no sideband) retain
          // the local cursor over the in-frame captured cursor.
          style={{
            display: 'block',
            cursor: cursorSnapshot ? 'none' : undefined,
          }}
          aria-label="Remote desktop canvas"
        />
      </div>
      <DesktopRectOverlay ref={overlayRef} />
      <DesktopCursorOverlay x={cursor.x} y={cursor.y} visible={cursor.visible} />
      <DesktopCursorTrackOverlay
        canvas={canvasRef}
        viewport={viewportRef}
        snapshot={cursorSnapshot}
      />
    </div>
  )
}
