// Where the remote-screen image actually sits inside the on-screen canvas
// element. Both desktop views (JPEG + H.264) style the canvas with
// `object-fit: contain` and a conditional `object-position` (centre when
// the keyboard is closed, bottom when open). Code that maps pointer coords
// to remote coords (or clamps the trackpad cursor to the visible image)
// needs the rect of the painted area, not the rect of the canvas box —
// taps in the letterbox bars must not register as remote-screen positions
// and the trackpad cursor must not drift into the bars.
//
// We read `object-position` from the computed style each call so the
// painted rect always tracks whatever the CSS currently says — no need to
// plumb anchor state through every consumer.

export interface PaintedRect {
  left: number
  top: number
  width: number
  height: number
}

function parseAnchor(value: string | undefined, fallback: number): number {
  if (!value) return fallback
  const v = value.trim()
  if (v.endsWith('%')) {
    const n = parseFloat(v)
    return Number.isFinite(n) ? n / 100 : fallback
  }
  // Named keywords are normalised by the browser to %s before reaching
  // getComputedStyle, but handle the common ones defensively.
  if (v === 'left' || v === 'top') return 0
  if (v === 'right' || v === 'bottom') return 1
  if (v === 'center') return 0.5
  return fallback
}

export function paintedRect(el: HTMLElement): PaintedRect {
  const r = el.getBoundingClientRect()
  const iw = (el as HTMLCanvasElement).width ?? 0
  const ih = (el as HTMLCanvasElement).height ?? 0
  if (iw <= 0 || ih <= 0 || r.width <= 0 || r.height <= 0) {
    return { left: r.left, top: r.top, width: r.width, height: r.height }
  }
  const boxAspect = r.width / r.height
  const intAspect = iw / ih
  // object-position: "<x> <y>" — both halves are %s after style resolution
  // ("center bottom" → "50% 100%"). Default to centre if anything's off.
  const op = getComputedStyle(el).objectPosition || '50% 50%'
  const parts = op.trim().split(/\s+/)
  const xPct = parseAnchor(parts[0], 0.5)
  const yPct = parseAnchor(parts[1], 0.5)
  if (intAspect > boxAspect) {
    // Intrinsic wider → fit width, letterbox top/bottom. Vertical anchor
    // (yPct) decides where the painted area sits inside the box.
    const height = r.width / intAspect
    return { left: r.left, top: r.top + (r.height - height) * yPct, width: r.width, height }
  }
  // Intrinsic taller → fit height, pillarbox left/right. Horizontal
  // anchor (xPct) decides the painted area's left position.
  const width = r.height * intAspect
  return { left: r.left + (r.width - width) * xPct, top: r.top, width, height: r.height }
}

/** Map a viewport pixel coord onto the remote screen's 0..1 space using the
 *  painted-rect (so taps in the letterbox bars clamp to the nearest edge
 *  of the remote screen instead of being scattered across its middle). */
export function clientToRemote(canvas: HTMLElement, clientX: number, clientY: number) {
  const r = paintedRect(canvas)
  if (r.width <= 0 || r.height <= 0) return { x: 0, y: 0 }
  return {
    x: Math.max(0, Math.min(1, (clientX - r.left) / r.width)),
    y: Math.max(0, Math.min(1, (clientY - r.top) / r.height)),
  }
}
