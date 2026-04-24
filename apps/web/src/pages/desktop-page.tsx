// Remote desktop page — lazy-loaded route at /h/:hostId/desktop.
// Fetches capabilities first; if unavailable renders a helpful message.
// Wires OffscreenCanvas worker (with main-thread fallback), WebRTC session,
// input handlers, toolbar, gesture help sheet, and reconnect modal.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'
import { useDesktopSession, type QualityTier } from '../hooks/use-desktop-session'
import { useDesktopInput, type InputMode } from '../hooks/use-desktop-input'
import DesktopToolbar from '../components/desktop-toolbar'
import DesktopGestureHelp from '../components/desktop-gesture-help'
import ReconnectModal from '../components/reconnect-modal'

interface Capabilities {
  available: boolean
  quality_tiers: QualityTier[]
  monitors: { id: number; label: string; width: number; height: number }[]
}

// Feature-detect OffscreenCanvas — Safari <16.4 lacks it.
const supportsOffscreen = typeof OffscreenCanvas !== 'undefined'

export default function DesktopPage() {
  const { hostId = '' } = useParams<{ hostId: string }>()
  const [caps, setCaps] = useState<Capabilities | null>(null)
  const [capsError, setCapsError] = useState('')
  const [deviceId, setDeviceId] = useState<string | null>(null)
  const [quality, setQuality] = useState<QualityTier>('med')
  const [inputMode, setInputMode] = useState<InputMode>('touch')
  const [showHelp, setShowHelp] = useState(false)

  const canvasRef = useRef<HTMLCanvasElement>(null)
  // Worker ref — null when running main-thread fallback
  const workerRef = useRef<Worker | null>(null)
  // Main-thread 2D context for OffscreenCanvas fallback
  const ctx2dRef = useRef<CanvasRenderingContext2D | null>(null)
  const workerInitialized = useRef(false)

  // ── Resolve authenticated device_id from /api/me ────────────────────────
  // The agent binds each WS to the session's device_id; we must send that
  // exact value or the handshake 403s. Sourcing client-side would create a
  // trust gap — see phase-04 review C-1.
  useEffect(() => {
    fetch('/api/me', { credentials: 'include' })
      .then((r) => (r.ok ? (r.json() as Promise<{ device_id: string }>) : null))
      .then((data) => { if (data?.device_id) setDeviceId(data.device_id) })
      .catch(() => {})
  }, [])

  // ── Fetch capabilities ──────────────────────────────────────────────────
  useEffect(() => {
    if (!hostId) return
    fetch(`/api/hosts/${hostId}/desktop/capabilities`, { credentials: 'include' })
      .then((r) => {
        if (!r.ok) throw new Error(`${r.status}`)
        return r.json() as Promise<Capabilities>
      })
      .then(setCaps)
      .catch((e: unknown) => setCapsError(String(e)))
  }, [hostId])

  // ── Tile callback — forwarded to worker or handled on main thread ───────
  const onTile = useCallback((buf: ArrayBuffer) => {
    if (buf.byteLength < 5) return
    const view = new DataView(buf)
    const tileX = view.getUint16(0, false)
    const tileY = view.getUint16(2, false)
    const flags = view.getUint8(4)
    const lastTile = (flags & 1) === 1

    if (supportsOffscreen && workerRef.current) {
      // Copy just the JPEG payload into its own ArrayBuffer so we can transfer
      // it zero-copy to the worker (buf itself may be from a pooled WS frame).
      const jpeg = new Uint8Array(buf.slice(5))
      workerRef.current.postMessage(
        { type: 'tile', tileX, tileY, jpeg, lastTile, frameTs: performance.now() },
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

  const { status, sendInput, setQuality: setSessionQuality, disconnect, attempt, screenDims } =
    useDesktopSession(hostId, deviceId ?? '', onTile, quality)

  // ── Init canvas worker once caps are available ──────────────────────────
  useEffect(() => {
    if (!caps?.available || workerInitialized.current || !canvasRef.current) return
    workerInitialized.current = true

    // Initial canvas size is a placeholder — the agent's `capabilities` WS
    // message arrives with the real encoder-output dims shortly after DC
    // open, and the effect below resizes the canvas / worker accordingly.
    const monitor = caps.monitors[0]
    canvasRef.current.width = monitor?.width ?? 1920
    canvasRef.current.height = monitor?.height ?? 1080

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

    // No cleanup: `transferControlToOffscreen` is a one-shot per canvas and
    // React 19 StrictMode double-invokes effects in dev — terminating here
    // kills the worker that owns the (already-transferred) canvas, and the
    // re-run can't rebuild it (second transfer throws). Leaving the worker
    // alive is safe: it becomes unreachable on real unmount and is GC'd.
  }, [caps])

  // ── Resize canvas to real encoder output on capabilities / tier change ──
  // Server emits `capabilities` once on connect and again on every tier
  // change, so the grid stays aligned with `tileX*128` / `tileY*128` writes.
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

  // ── Quality change propagates to session ────────────────────────────────
  function handleQualityChange(tier: QualityTier) {
    setQuality(tier)
    setSessionQuality(tier)
  }

  // ── Unavailable state ───────────────────────────────────────────────────
  if (capsError) {
    return (
      <div className="flex items-center justify-center h-full p-6">
        <div className="text-text-muted text-sm">Failed to load desktop capabilities.</div>
      </div>
    )
  }

  if (caps && !caps.available) {
    return (
      <div className="flex items-center justify-center h-full p-6">
        <div className="max-w-sm text-center space-y-2">
          <div className="text-text-primary font-medium">Remote Desktop not available</div>
          <div className="text-text-muted text-sm">
            Enable Screen Recording permission in System Settings, then restart the agent.
          </div>
        </div>
      </div>
    )
  }

  if (!caps) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        Loading…
      </div>
    )
  }

  const showReconnect =
    status === 'reconnecting' || (status === 'disconnected' && attempt >= 3)

  return (
    <div className="relative flex flex-col lg:flex-row w-full h-full overflow-hidden bg-black">
      {/* Canvas — fills all available space */}
      <div className="flex-1 relative min-h-0">
        <CanvasWithInput
          canvasRef={canvasRef}
          mode={inputMode}
          sendInput={sendInput}
        />
      </div>

      {/* Toolbar — fixed bottom on mobile, right sidebar on lg+ */}
      <div className="fixed bottom-0 left-0 right-0 z-20 lg:static lg:w-60 lg:h-full lg:flex-shrink-0">
        <DesktopToolbar
          quality={quality}
          onQualityChange={handleQualityChange}
          inputMode={inputMode}
          onInputModeToggle={() => setInputMode((m) => (m === 'touch' ? 'trackpad' : 'touch'))}
          onKeyEvent={sendInput}
          onShowGestureHelp={() => setShowHelp(true)}
        />
      </div>

      <DesktopGestureHelp open={showHelp} onClose={() => setShowHelp(false)} />

      <ReconnectModal
        open={showReconnect}
        attempt={attempt}
        maxAttempts={3}
        exhausted={status === 'disconnected'}
        onCancel={disconnect}
      />
    </div>
  )
}

// Extracted to keep line count down and satisfy the useDesktopInput hook
// signature (canvas ref + mode + sendInput must be stable across renders).
function CanvasWithInput({
  canvasRef,
  mode,
  sendInput,
}: {
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  mode: InputMode
  sendInput: (ev: Parameters<typeof useDesktopInput>[0]['sendInput'] extends (ev: infer E) => void ? E : never) => void
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
