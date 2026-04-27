import { useEffect, useRef, useState } from 'react'
import Button from '../ui/button'
import { useToast } from '../ui/use-toast'

interface Props {
  onDetect: (key: string) => void
  onClose: () => void
}

// Extract ?k= param from QR URL, or return the raw value if it's already a plain token.
function extractKey(raw: string): string | null {
  try {
    const url = new URL(raw)
    const k = url.searchParams.get('k')
    if (k) return k
  } catch {
    // Not a URL — treat raw value as the token directly
  }
  // Accept a plain hex/alnum token that looks like an OTK or pairing code
  const stripped = raw.replace(/[\s-]/g, '')
  if (stripped.length >= 6) return stripped
  return null
}

export default function QrScanModal({ onDetect, onClose }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const streamRef = useRef<MediaStream | null>(null)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const [permDenied, setPermDenied] = useState(false)
  const toast = useToast()

  useEffect(() => {
    let cancelled = false

    async function start() {
      let stream: MediaStream
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: 'environment' },
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
      if (videoRef.current) {
        videoRef.current.srcObject = stream
        void videoRef.current.play()
      }

      // @ts-expect-error BarcodeDetector is not yet in TS lib
      const detector = new window.BarcodeDetector({ formats: ['qr_code'] })

      timerRef.current = setInterval(async () => {
        if (!videoRef.current || videoRef.current.readyState < 2) return
        try {
          const codes = (await detector.detect(videoRef.current)) as Array<{ rawValue: string }>
          if (codes.length > 0) {
            const key = extractKey(codes[0].rawValue)
            if (key) {
              cleanup()
              onDetect(key)
            }
          }
        } catch {
          // Detection errors are transient — keep polling
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
      onClick={(e) => { if (e.target === e.currentTarget) onClose() }}
    >
      <div className="relative w-full max-w-xs mx-4 rounded-xl overflow-hidden bg-black shadow-2xl">
        {/* Close button */}
        <button
          type="button"
          onClick={onClose}
          aria-label="Close QR scanner"
          className="absolute top-2 right-2 z-10 w-8 h-8 flex items-center justify-center rounded-full bg-black/50 text-white hover:bg-black/70 transition-colors"
        >
          <svg viewBox="0 0 24 24" className="w-4 h-4" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>

        {/* Camera viewport */}
        <video
          ref={videoRef}
          muted
          playsInline
          className="w-full aspect-square object-cover"
        />

        {/* Scan overlay guide */}
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="w-48 h-48 border-2 border-white/60 rounded-lg" />
        </div>

        <div className="px-4 py-3 bg-black/80 text-center">
          <p className="text-xs text-white/70">Point at the QR code shown on your host</p>
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
