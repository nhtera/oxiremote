// Local virtual cursor for trackpad mode. SPA-side rendering because the
// streamed remote cursor is unreliable: ScreenCaptureKit on macOS 14+
// composites a cursor only when `showsCursor` is set (default true) and even
// then lock screens / app full-screen states can suppress it. A client-side
// sprite gives the user immediate "where will my tap land" feedback even
// when the remote frame has no cursor pixels.
//
// Shape mirrors the native macOS arrow — 16 px, black fill, thin white
// outline. The arrow tip is anchored to (x, y) so the click position
// matches the visible point of the arrow.

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
        // Tip anchor: SVG path starts at (1, 1) in viewbox space — translate
        // -1 px so the tip aligns precisely with the stored coord.
        transform: 'translate(-1px, -1px)',
        filter: 'drop-shadow(0 1px 1px rgba(0, 0, 0, 0.45))',
      }}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
        <path
          d="M1 1 L1 12 L4.2 9.2 L6.4 13.6 L7.8 13 L5.6 8.6 L10 8.6 Z"
          fill="#000000"
          stroke="#ffffff"
          strokeWidth="1.2"
          strokeLinejoin="round"
        />
      </svg>
    </div>
  )
}
