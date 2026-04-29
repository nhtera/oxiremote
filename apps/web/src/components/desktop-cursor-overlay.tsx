// Local virtual cursor for trackpad mode. Positioned absolutely inside the
// viewport (transform layer's parent) so it tracks finger delta with zero
// latency — the remote's cursor follows asynchronously, but the user always
// sees the local arrow respond immediately.
//
// Shape mirrors the macOS / Windows pointer (filled black with white outline)
// so it reads as a cursor at any background colour.

interface Props {
  x: number
  y: number
  visible: boolean
}

export default function DesktopCursorOverlay({ x, y, visible }: Props) {
  if (!visible) return null
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute z-30"
      style={{
        left: x,
        top: y,
        // Translate so the arrow tip sits on (x, y), not the bounding box.
        transform: 'translate(-2px, -2px)',
        // Drop shadow lifts the arrow off bright remote frames.
        filter: 'drop-shadow(0 1px 2px rgba(0, 0, 0, 0.6))',
      }}
    >
      <svg width="20" height="22" viewBox="0 0 20 22" fill="none" xmlns="http://www.w3.org/2000/svg">
        <path
          d="M2 2 L2 17 L6 13 L9 19 L11 18 L8 12 L14 12 Z"
          fill="#ffffff"
          stroke="#0a0a0a"
          strokeWidth="1.4"
          strokeLinejoin="round"
        />
      </svg>
    </div>
  )
}
