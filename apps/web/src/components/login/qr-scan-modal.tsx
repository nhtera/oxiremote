import { useEffect, useRef, useState } from 'react'
import Button from '../ui/button'
import { useToast } from '../ui/use-toast'

interface Props {
  onDetect: (key: string) => void
  onClose: () => void
}

// Live camera + jsQR scan. Works everywhere getUserMedia + HTTPS works,
// including iOS Safari (no BarcodeDetector needed). Photographing a QR
// on a screen produces moiré/glare that jsQR can't read, so we scan
// video frames continuously instead of decoding a single still photo.

// Extract ?k= param from QR URL, or return the raw value if it's already
// a plain token. The login page handles `?k=` deep-link auto-submit.
function extractKey(raw: string): string | null {
  try {
    const url = new URL(raw)
    const k = url.searchParams.get('k')
    if (k) return k
  } catch {
    // Not a URL — treat raw value as the token directly
  }
  const stripped = raw.replace(/[\s-]/g, '')
  if (stripped.length >= 6) return stripped
  return null
}

export default function QrScanModal({ onDetect, onClose }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const streamRef = useRef<MediaStream | null>(null)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const detectedRef = useRef(false)
  const [permDenied, setPermDenied] = useState(false)
  const [starting, setStarting] = useState(true)
  const toast = useToast()

  useEffect(() => {
    let cancelled = false
    const canvas = document.createElement('canvas')
    const ctx = canvas.getContext('2d', { willReadFrequently: true })

    async function start() {
      let stream: MediaStream
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          video: {
            facingMode: { ideal: 'environment' },
            width: { ideal: 1280 },
            height: { ideal: 720 },
          },
          audio: false,
        })
      } catch {
        if (!cancelled) {
          setPermDenied(true)
          toast.show({ kind: 'danger', title: 'Camera permission denied' })
          onClose()
        }
        return
      }
      if (cancelled) {
        stream.getTracks().forEach((t) => t.stop())
        return
      }
      streamRef.current = stream

      const video = videoRef.current
      if (!video) return
      video.srcObject = stream
      try {
        await video.play()
      } catch {
        // Autoplay can fail if the modal was opened without a user gesture;
        // user can tap the video to retry below.
      }
      if (!cancelled) setStarting(false)

      // Lazy-load jsQR so it doesn't bloat the initial route bundle.
      const { default: jsQR } = await import('jsqr')

      timerRef.current = setInterval(() => {
        if (cancelled || detectedRef.current) return
        const v = videoRef.current
        if (!v || !ctx || v.readyState < 2) return
        const w = v.videoWidth
        const h = v.videoHeight
        if (w === 0 || h === 0) return

        if (canvas.width !== w) canvas.width = w
        if (canvas.height !== h) canvas.height = h
        try {
          ctx.drawImage(v, 0, 0, w, h)
          const img = ctx.getImageData(0, 0, w, h)
          // dontInvert: host QR is always normal black-on-white — skips the
          // expensive inverted-pass jsQR runs by default.
          const code = jsQR(img.data, img.width, img.height, {
            inversionAttempts: 'dontInvert',
          })
          if (!code) return
          const key = extractKey(code.data)
          if (!key) return
          detectedRef.current = true
          cleanup()
          onDetect(key)
        } catch {
          // Decode errors are transient — keep polling
        }
      }, 250)
    }

    void start()

    function cleanup() {
      cancelled = true
      if (timerRef.current) clearInterval(timerRef.current)
      streamRef.current?.getTracks().forEach((t) => t.stop())
      streamRef.current = null
    }

    return cleanup
    // onDetect / onClose are stable refs from parent — intentionally omitted
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  if (permDenied) return null

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Scan QR code"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="relative w-full max-w-xs mx-4 rounded-xl overflow-hidden bg-black shadow-2xl">
        <button
          type="button"
          onClick={onClose}
          aria-label="Close QR scanner"
          className="absolute top-2 right-2 z-10 w-8 h-8 flex items-center justify-center rounded-full bg-black/50 text-white hover:bg-black/70 transition-colors"
        >
          <svg
            viewBox="0 0 24 24"
            className="w-4 h-4"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
        <div className="relative">
          <video
            ref={videoRef}
            muted
            playsInline
            autoPlay
            onClick={() => void videoRef.current?.play()}
            className="w-full aspect-square object-cover bg-black"
          />
          <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
            <div className="w-48 h-48 border-2 border-white/60 rounded-lg" />
          </div>
          {starting && (
            <div className="absolute inset-0 flex items-center justify-center text-xs text-white/70">
              Starting camera…
            </div>
          )}
        </div>
        <div className="px-4 py-3 bg-black/80 text-center">
          <p className="text-xs text-white/70">
            Point at the QR code shown on your host
          </p>
          <Button
            variant="ghost"
            size="sm"
            onClick={onClose}
            className="mt-2 text-white/60 hover:text-white border-white/20"
          >
            Cancel
          </Button>
        </div>
      </div>
    </div>
  )
}
