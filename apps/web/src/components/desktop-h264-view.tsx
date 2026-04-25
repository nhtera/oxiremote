// H.264 over WebRTC desktop view — Phase 03 pipeline.
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
import { useDesktopInput, type InputMode } from '../hooks/use-desktop-input'

interface SessionSnapshot {
  status: DesktopStatus
  attempt: number
  screenDims?: { width: number; height: number }
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  disconnect: () => void
}

interface Props {
  hostId: string
  deviceId: string
  quality: QualityTier
  inputMode: InputMode
  hidpi: boolean
  smoothScaling: boolean
  monitorDefault?: { width: number; height: number }
  onSessionChange: (s: SessionSnapshot) => void
  onSessionApi: (api: SessionApi) => void
}

export default function DesktopH264View({
  hostId,
  deviceId,
  quality,
  inputMode,
  hidpi,
  smoothScaling,
  monitorDefault,
  onSessionChange,
  onSessionApi,
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

  const { status, sendInput, setQuality, setSettings, disconnect, attempt, screenDims } =
    useDesktopVideoSession(hostId, deviceId, onFrame, quality, hidpi)

  useEffect(() => {
    onSessionChange({ status, attempt, screenDims })
  }, [status, attempt, screenDims, onSessionChange])

  useEffect(() => {
    onSessionApi({ sendInput, setQuality, setSettings, disconnect })
  }, [sendInput, setQuality, setSettings, disconnect, onSessionApi])

  // Set an initial canvas size from the monitor hint so the layout doesn't
  // pop when the first frame arrives; the onFrame loop adjusts precisely.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || canvas.width !== 300 /* default */) return
    canvas.width = monitorDefault?.width ?? 1920
    canvas.height = monitorDefault?.height ?? 1080
  }, [monitorDefault])

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
