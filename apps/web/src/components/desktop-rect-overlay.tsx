import { forwardRef, useImperativeHandle, useRef, useState } from 'react'

// Visual feedback for rectangle-select mode. The drag itself goes through the
// same mouse-down → move → up that a regular drag uses; this overlay just
// gives the user a dashed outline so they can see what they're sweeping over.
//
// The parent (a desktop view) calls show()/hide() through the ref. Coords
// are in viewport pixels — the overlay is positioned via fixed so it sits
// above the canvas regardless of layout.

export interface DesktopRectOverlayHandle {
  show: (start: { x: number; y: number }, end: { x: number; y: number }) => void
  hide: () => void
}

interface Rect {
  left: number
  top: number
  width: number
  height: number
}

const DesktopRectOverlay = forwardRef<DesktopRectOverlayHandle>(function DesktopRectOverlay(
  _props,
  ref,
) {
  const [rect, setRect] = useState<Rect | null>(null)
  const lastRef = useRef<Rect | null>(null)

  useImperativeHandle(ref, () => ({
    show(start, end) {
      const next: Rect = {
        left: Math.min(start.x, end.x),
        top: Math.min(start.y, end.y),
        width: Math.abs(end.x - start.x),
        height: Math.abs(end.y - start.y),
      }
      lastRef.current = next
      setRect(next)
    },
    hide() {
      lastRef.current = null
      setRect(null)
    },
  }))

  if (!rect) return null
  return (
    <div
      aria-hidden
      className="pointer-events-none fixed z-40 border border-dashed border-white/90 bg-white/5"
      style={{
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      }}
    />
  )
})

export default DesktopRectOverlay
