// H.264 over WebRTC desktop view.
//
// Mounts `useDesktopVideoSession` which owns a hidden <video> sink for the
// incoming MediaStreamTrack and invokes `onFrame` once per decoded frame
// (via requestVideoFrameCallback, rAF fallback). We blit the frame to the
// on-screen canvas with `drawImage(video, ...)`. Session state + API are
// pushed up so the parent's shared shell (toolbar, reconnect modal) stays
// DRY with the JPEG view.

import { useCallback, useEffect, useRef } from 'react'
import { useDesktopVideoSession } from '../hooks/use-desktop-video-session'
import type {
  DesktopInputEvent,
  DesktopStatus,
  QualityTier,
} from '../hooks/use-desktop-session'
import { useDesktopInput, type InputMode, type GestureMode } from '../hooks/use-desktop-input'
import { useCanvasGestures } from '../hooks/use-canvas-gestures'
import { captureCanvas } from '../lib/desktop-screenshot'
import DesktopRectOverlay, { type DesktopRectOverlayHandle } from './desktop-rect-overlay'
import DesktopCursorOverlay from './desktop-cursor-overlay'

interface SessionSnapshot {
  status: DesktopStatus
  attempt: number
  screenDims?: { width: number; height: number }
  pipelineInfo?: import('../hooks/use-desktop-session').DesktopPipelineInfo
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  disconnect: () => void
  screenshot?: () => Promise<void>
}

/** Imperative handle exposed by the canvas's pinch/pan gesture stack. */
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
  hostLabel: string
  onSessionChange: (s: SessionSnapshot) => void
  onSessionApi: (api: SessionApi) => void
  onZoomChange?: (scale: number) => void
  onGestureApi?: (api: DesktopGestureApi) => void
  /** See JPEG view: bottom-anchor only while the soft keyboard is open. */
  bottomAnchor?: boolean
}

export default function DesktopH264View({
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
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const ctx2dRef = useRef<CanvasRenderingContext2D | null>(null)
  // Read latest smoothScaling inside the rVFC callback without recreating
  // onFrame (which would tear down + restart the rVFC handle on each toggle).
  const smoothRef = useRef(smoothScaling)
  useEffect(() => {
    smoothRef.current = smoothScaling
    // Apply immediately to the live context so the next frame paints with
    // the new filter — without waiting for the next canvas resize.
    if (ctx2dRef.current) ctx2dRef.current.imageSmoothingEnabled = smoothScaling
  }, [smoothScaling])

  // Draw each decoded frame to the on-screen canvas. The hook hands us the
  // hidden <video> element so we can read `videoWidth`/`videoHeight` — the
  // stream dimensions mutate on quality-tier changes and we must resize
  // before the next drawImage to avoid a stale-sized canvas.
  const onFrame = useCallback((_frame: unknown, video: HTMLVideoElement) => {
    const canvas = canvasRef.current
    if (!canvas) return
    let ctx = ctx2dRef.current
    if (!ctx) {
      ctx = canvas.getContext('2d')
      ctx2dRef.current = ctx
      // Screen content is text-heavy: default OFF keeps glyph edges crisp
      // when CSS upscales the canvas. Users who view downscaled (e.g. mobile)
      // can flip Smooth Scaling on to soften aliasing.
      if (ctx) ctx.imageSmoothingEnabled = smoothRef.current
    }
    if (!ctx) return
    const w = video.videoWidth
    const h = video.videoHeight
    if (!w || !h) return
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w
      canvas.height = h
      // Resizing a canvas resets context state — re-apply the filter.
      ctx.imageSmoothingEnabled = smoothRef.current
    }
    ctx.drawImage(video, 0, 0, w, h)
  }, [])

  const { status, sendInput, setQuality, setSettings, disconnect, attempt, screenDims, pipelineInfo } =
    useDesktopVideoSession(hostId, deviceId, onFrame, quality, hidpi)

  useEffect(() => {
    onSessionChange({ status, attempt, screenDims, pipelineInfo })
  }, [status, attempt, screenDims, pipelineInfo, onSessionChange])

  // The H.264 view paints to a main-thread canvas (drawImage from <video>),
  // so toBlob can read pixels directly — no worker round-trip.
  const screenshot = useCallback(async () => {
    if (canvasRef.current) {
      await captureCanvas(canvasRef.current, { label: hostLabel })
    }
  }, [hostLabel])

  useEffect(() => {
    onSessionApi({ sendInput, setQuality, setSettings, disconnect, screenshot })
  }, [sendInput, setQuality, setSettings, disconnect, screenshot, onSessionApi])

  // Set an initial canvas size from the monitor hint so the layout doesn't
  // pop when the first frame arrives; the onFrame loop adjusts precisely.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || canvas.width !== 300 /* default */) return
    canvas.width = monitorDefault?.width ?? 1920
    canvas.height = monitorDefault?.height ?? 1080
  }, [monitorDefault])

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
}: {
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  mode: InputMode
  gestureMode: GestureMode
  sendInput: (ev: DesktopInputEvent) => void
  onZoomChange?: (scale: number) => void
  onGestureApi?: (api: DesktopGestureApi) => void
  bottomAnchor: boolean
  screenDims?: { width: number; height: number }
}) {
  const overlayRef = useRef<DesktopRectOverlayHandle>(null)
  const layerRef = useRef<HTMLDivElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
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
  const { cursor, resetZoom, fitToViewport } = useCanvasGestures({
    target: canvasRef,
    layer: layerRef,
    viewport: viewportRef,
    mode,
    sendInput,
    onZoomChange,
    disabled: gestureMode === 'rect',
  })
  useEffect(() => {
    onGestureApi?.({ resetZoom })
  }, [onGestureApi, resetZoom])
  // Smart auto-fit on first valid stream dims (mirrors the JPEG view).
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
          // See JPEG-view comment — bottom-anchor only while the soft
          // keyboard is open; centred otherwise so the image sits
          // balanced when the operator isn't typing.
          className={`w-full h-full object-contain outline-none ${bottomAnchor ? 'object-bottom' : ''}`}
          style={{ display: 'block' }}
          aria-label="Remote desktop canvas"
        />
      </div>
      <DesktopRectOverlay ref={overlayRef} />
      <DesktopCursorOverlay x={cursor.x} y={cursor.y} visible={cursor.visible} />
    </div>
  )
}
