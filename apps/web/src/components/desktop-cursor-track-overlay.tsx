// Renders the host OS cursor inside the painted-rect of the remote desktop
// canvas, driven by pose + shape snapshots from the `cursor` WebRTC
// sideband DataChannel (see `lib/desktop-cursor-track.ts` and
// `agent/src/cursor_sideband.rs`).
//
// Why not just look at the streamed video frame? Because the cursor would
// then lag by one or more frame intervals (~33-100 ms on H.264 High). The
// sideband path renders at network RTT — pose updates flow at 60 Hz over
// SCTP, independent of the encoder cadence. CRD and RustDesk use the same
// pattern for the same reason.
//
// The sprite cache is owned by `attachCursorChannel`; this component
// receives a flat `(pose, sprite)` snapshot already resolved, so it never
// reads a ref during render.

import { useEffect, useRef, type RefObject } from 'react'
import type { CursorSnapshot } from '../lib/desktop-cursor-track'
import { paintedRect } from '../lib/canvas-painted-rect'

interface Props {
  /** The on-screen canvas displaying the remote desktop. We need its
   *  painted-rect (object-contain letterbox-aware) to translate the
   *  server's normalised `[0..1]` pose into pixel coords inside the
   *  viewport. */
  canvas: RefObject<HTMLCanvasElement | null>
  /** The viewport (canvas's transform-parent). Cursor position is
   *  rendered as `absolute` inside this element. */
  viewport: RefObject<HTMLElement | null>
  /** Latest pose + matching sprite. Null until the first pose lands. */
  snapshot: CursorSnapshot | null
}

export default function DesktopCursorTrackOverlay({
  canvas,
  viewport,
  snapshot,
}: Props) {
  const canvasEl = useRef<HTMLCanvasElement | null>(null)
  // Re-upload the sprite RGBA only when the id changes — pose ticks
  // arrive 60 Hz, the bitmap is the same on every one until the user's
  // cursor crosses a shape boundary (text I-beam, busy, resize, …).
  const lastRenderedIdRef = useRef<number | null>(null)

  useEffect(() => {
    if (!snapshot || snapshot.pose.hidden || !snapshot.sprite) return
    const sprite = snapshot.sprite
    if (lastRenderedIdRef.current === sprite.id) return
    const el = canvasEl.current
    if (!el) return
    el.width = sprite.width
    el.height = sprite.height
    const ctx = el.getContext('2d')
    if (!ctx) return
    const data = ctx.createImageData(sprite.width, sprite.height)
    data.data.set(sprite.rgba)
    ctx.putImageData(data, 0, 0)
    lastRenderedIdRef.current = sprite.id
  }, [snapshot])

  if (!snapshot || snapshot.pose.hidden || !snapshot.sprite) return null

  const c = canvas.current
  const v = viewport.current
  if (!c || !v) return null
  const pr = paintedRect(c)
  const vr = v.getBoundingClientRect()
  // The canvas's pixel width is the encoded-frame width; `pr.width` is
  // the painted size in CSS pixels. Ratio gives us the screen scale we
  // apply to both position offset and sprite size.
  const canvasW = c.width || pr.width
  const scale = canvasW > 0 ? pr.width / canvasW : 1
  const { pose, sprite } = snapshot
  const x = pr.left - vr.left + pose.x * pr.width - sprite.hotspotX * scale
  const y = pr.top - vr.top + pose.y * pr.height - sprite.hotspotY * scale

  return (
    <canvas
      ref={canvasEl}
      aria-hidden
      className="pointer-events-none absolute z-30"
      style={{
        left: x,
        top: y,
        width: sprite.width * scale,
        height: sprite.height * scale,
        imageRendering: 'pixelated',
      }}
    />
  )
}
