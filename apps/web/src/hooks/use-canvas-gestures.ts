// Pointer Events gesture state machine for the remote-desktop canvas.
//
// Replaces the legacy 1-finger-as-mouse-drag handler in use-desktop-input.ts —
// that handler emitted mousedown after 4 px of finger movement, which made
// every accidental swipe across the remote text into a text selection. This
// hook follows Chrome Remote Desktop's mobile model instead:
//
//   Touch mode (default):
//     - 1-finger drag = pan the local canvas (zero remote input)
//     - 1-finger tap (no drag) = single click at finger position
//     - 1-finger long-press (>500 ms, no drag) = ARM left-button drag — the
//       next finger movement emits remote mousedown/move/up so the user can
//       intentionally select / drag a remote window
//     - 2-finger tap = right click at midpoint
//     - 2-finger pinch = zoom canvas about the finger midpoint
//     - 2-finger pan = pan canvas (delegates to centroid translation)
//
//   Trackpad mode:
//     - 1-finger drag = move a local virtual cursor (no immediate remote
//       traffic; cursor delta is applied locally + sent as a remote
//       mouse-move so the host's cursor follows)
//     - 1-finger tap (no drag) = click at the virtual cursor's position
//     - 1-finger long-press = right-click at cursor position
//     - 2-finger drag = scroll wheel
//     - 2-finger pinch = zoom canvas (same as touch mode)
//
// Coords sent to the agent are normalized 0..1 against the canvas's PAINTED
// rect (the object-contain letterboxed area inside the canvas box) — not the
// box itself. The DOM `<canvas>` element fills its parent (`w-full h-full`),
// but `object-contain` paints the actual frame letterboxed inside that box.
// Mapping against the box would route taps in the black bars to the middle
// of the remote screen, and taps near the visible top of the image to ~27%
// down on the remote — the original cursor-position bug. We centre the
// virtual cursor on the painted rect for the same reason.

import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'
import type { DesktopInputEvent } from './use-desktop-session'

export type InputMode = 'touch' | 'trackpad'

interface Args {
  /** The actual <canvas> / <video> element. Used for coord normalization. */
  target: RefObject<HTMLElement | null>
  /** The element that receives `transform: matrix(...)`. Usually the wrapper
   *  around the canvas. Pointer listeners are attached here too. */
  layer: RefObject<HTMLElement | null>
  /** The viewport (transform layer's parent) — used to size the virtual
   *  cursor's clamp box and read pinch midpoints. */
  viewport: RefObject<HTMLElement | null>
  mode: InputMode
  sendInput: (ev: DesktopInputEvent) => void
  /** Optional zoom-change callback so the toolbar can show a zoom %. */
  onZoomChange?: (scale: number) => void
  /** When true the hook unbinds — used by the rect-marquee gesture mode so
   *  the legacy touch-event handler in `use-desktop-input.ts` can run instead. */
  disabled?: boolean
}

/** Trackpad-mode virtual cursor position, in viewport-local pixels. The
 *  consumer renders a sprite at (x, y) when `visible` is true; a tap without
 *  drag fires a click at this position rather than at the finger. */
interface CursorState {
  x: number
  y: number
  visible: boolean
}

const LONG_PRESS_MS = 500
const PAN_THRESHOLD_PX = 8
const TAP_MAX_MS = 300
const MIN_SCALE = 1
const MAX_SCALE = 4
// Trackpad-mode sensitivity multiplier. CRD/MS-RD ship 1.0× by default and
// the most common user complaint is "cursor feels too slow" (e.g. RustDesk
// discussion #12090). 1.5× lets a single finger swipe traverse most of the
// remote screen without the user feeling the cursor is dragging behind.
const TRACKPAD_SENSITIVITY = 1.5

/** Compute the canvas's painted rect (object-contain letterboxed area) inside
 *  its bounding box. Reads `width`/`height` attributes for intrinsic dims —
 *  both JPEG and H.264 views resize the canvas attribute to the stream dims
 *  so this is reliable. Falls back to the bounding rect when intrinsics are
 *  unknown (first frame race). */
function paintedRect(el: HTMLElement) {
  const r = el.getBoundingClientRect()
  const iw = (el as HTMLCanvasElement).width ?? 0
  const ih = (el as HTMLCanvasElement).height ?? 0
  if (iw <= 0 || ih <= 0 || r.width <= 0 || r.height <= 0) {
    return { left: r.left, top: r.top, width: r.width, height: r.height }
  }
  const boxAspect = r.width / r.height
  const intAspect = iw / ih
  if (intAspect > boxAspect) {
    // Intrinsic wider → fit width, letterbox top/bottom.
    const height = r.width / intAspect
    return { left: r.left, top: r.top + (r.height - height) / 2, width: r.width, height }
  }
  // Intrinsic taller → fit height, pillarbox left/right.
  const width = r.height * intAspect
  return { left: r.left + (r.width - width) / 2, top: r.top, width, height: r.height }
}

/** Normalize a viewport pixel coord to the canvas's 0..1 remote-screen space.
 *  Uses the painted rect so taps in the letterbox bars map to the nearest
 *  edge of the remote screen instead of being scattered across the middle. */
function clientToRemote(canvas: HTMLElement, clientX: number, clientY: number) {
  const r = paintedRect(canvas)
  if (r.width <= 0 || r.height <= 0) return { x: 0, y: 0 }
  return {
    x: Math.max(0, Math.min(1, (clientX - r.left) / r.width)),
    y: Math.max(0, Math.min(1, (clientY - r.top) / r.height)),
  }
}

interface PointSnapshot {
  clientX: number
  clientY: number
}

function dist(a: PointSnapshot, b: PointSnapshot) {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)
}

export function useCanvasGestures({
  target,
  layer,
  viewport,
  mode,
  sendInput,
  onZoomChange,
  disabled = false,
}: Args) {
  // Trackpad-mode virtual cursor. Mirrored into a ref so the rAF-driven
  // pointermove path can read the latest value without a closure rebuild,
  // while React re-renders the sprite on (x, y, visible) changes.
  const [cursor, setCursor] = useState<CursorState>({ x: 0, y: 0, visible: false })
  const cursorRef = useRef(cursor)
  useEffect(() => {
    cursorRef.current = cursor
  }, [cursor])

  // Hot-state refs — never trigger re-renders. The matrix is applied imperatively.
  const scaleRef = useRef(1)
  const txRef = useRef(0)
  const tyRef = useRef(0)

  // Map<pointerId, { clientX, clientY, startX, startY, startTime, moved }>
  type Pointer = {
    clientX: number
    clientY: number
    startX: number
    startY: number
    startTime: number
    moved: boolean
  }
  const pointersRef = useRef<Map<number, Pointer>>(new Map())
  // Pinch state held while exactly 2 pointers are down.
  const pinchRef = useRef<{ startDist: number; startScale: number; midX: number; midY: number } | null>(null)
  // Long-press timer for the touch-mode "drag arm" gesture.
  const longPressRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // True after a successful long-press: the next 1-finger move emits a remote
  // mousedown and the eventual pointerup emits mouseup.
  const dragArmedRef = useRef(false)
  // Trackpad-mode flag: while a single pointer is down, finger delta moves
  // the virtual cursor (no remote drag). Cleared on pointer-up.
  const trackpadDraggingRef = useRef(false)

  const modeRef = useRef(mode)
  useEffect(() => {
    modeRef.current = mode
    if (mode !== 'trackpad') {
      setCursor((c) => ({ ...c, visible: false }))
      return
    }
    // Park the cursor over the painted-image centre on every entry. The
    // canvas is `object-contain`-letterboxed inside the viewport; landing
    // in the bars would feel broken. Defer one rAF so layout has settled
    // (refs can return 0×0 on synchronous read after a route remount).
    const raf = requestAnimationFrame(() => {
      const c = target.current
      const v = viewport.current
      if (!c || !v) {
        setCursor((cur) => ({ ...cur, visible: true }))
        return
      }
      const pr = paintedRect(c)
      const vr = v.getBoundingClientRect()
      setCursor({
        x: pr.left - vr.left + pr.width / 2,
        y: pr.top - vr.top + pr.height / 2,
        visible: true,
      })
    })
    return () => cancelAnimationFrame(raf)
  }, [mode, target, viewport])

  const sendInputRef = useRef(sendInput)
  useEffect(() => {
    sendInputRef.current = sendInput
  }, [sendInput])

  /** Apply the current scale + translation to the transform layer. Called
   *  every time scale/tx/ty change — outside React because we don't want a
   *  re-render per frame of pinch-pan. */
  const applyTransform = useCallback(() => {
    const el = layer.current
    if (!el) return
    el.style.transform = `translate(${txRef.current}px, ${tyRef.current}px) scale(${scaleRef.current})`
    el.style.transformOrigin = '0 0'
    onZoomChange?.(scaleRef.current)
  }, [layer, onZoomChange])

  /** Send an absolute mouse event to the remote, scaled to 0..1. The
   *  `clientX`/`clientY` are the finger position in viewport pixels; the
   *  canvas's bounding rect handles the rest. */
  const sendMouse = useCallback(
    (action: 'down' | 'up' | 'move', btn: 'left' | 'right', clientX: number, clientY: number) => {
      const el = target.current
      if (!el) return
      const { x, y } = clientToRemote(el, clientX, clientY)
      sendInputRef.current({ t: 'mouse', action, btn, x, y })
    },
    [target],
  )

  /** Move-only variant — no button, used for hover updates in trackpad mode. */
  const sendMove = useCallback(
    (clientX: number, clientY: number) => {
      const el = target.current
      if (!el) return
      const { x, y } = clientToRemote(el, clientX, clientY)
      sendInputRef.current({ t: 'mouse', action: 'move', x, y })
    },
    [target],
  )

  // Bind a non-passive `gesturestart` blocker — `touch-action: none` doesn't
  // stop iOS Safari's pinch-zoom-page event family by itself.
  useEffect(() => {
    const block = (e: Event) => e.preventDefault()
    window.addEventListener('gesturestart', block, { passive: false })
    window.addEventListener('gesturechange', block, { passive: false })
    window.addEventListener('gestureend', block, { passive: false })
    return () => {
      window.removeEventListener('gesturestart', block)
      window.removeEventListener('gesturechange', block)
      window.removeEventListener('gestureend', block)
    }
  }, [])

  useEffect(() => {
    if (disabled) return
    const el = layer.current
    if (!el) return

    function clearLongPress() {
      if (longPressRef.current) {
        clearTimeout(longPressRef.current)
        longPressRef.current = null
      }
    }

    function onPointerDown(e: PointerEvent) {
      // Filter to touch/pen pointers — desktop mouse falls through to the
      // legacy useDesktopInput's mousemove handler, which keeps standard
      // click-and-drag behaviour where it's expected.
      if (e.pointerType !== 'touch' && e.pointerType !== 'pen') return
      // Suppress synthesised mouse events that iOS Safari fires after a
      // touch — without this, both this hook and useDesktopInput process
      // the same input.
      e.preventDefault()
      // setPointerCapture so we keep getting move/up even if the finger
      // leaves the layer's bounding box mid-gesture.
      try {
        ;(e.target as Element).setPointerCapture?.(e.pointerId)
      } catch {
        /* not all browsers / event targets support capture */
      }

      pointersRef.current.set(e.pointerId, {
        clientX: e.clientX,
        clientY: e.clientY,
        startX: e.clientX,
        startY: e.clientY,
        startTime: performance.now(),
        moved: false,
      })

      if (pointersRef.current.size === 2) {
        // Two fingers down → pinch state initialised. Cancel any pending
        // long-press from the first finger.
        clearLongPress()
        const [a, b] = Array.from(pointersRef.current.values())
        pinchRef.current = {
          startDist: dist(a, b),
          startScale: scaleRef.current,
          midX: (a.clientX + b.clientX) / 2,
          midY: (a.clientY + b.clientY) / 2,
        }
        return
      }

      // Single finger:
      if (modeRef.current === 'touch') {
        // Arm the long-press timer; if the finger doesn't move past
        // PAN_THRESHOLD_PX before LONG_PRESS_MS, switch to drag-armed mode.
        const startClientX = e.clientX
        const startClientY = e.clientY
        clearLongPress()
        longPressRef.current = setTimeout(() => {
          dragArmedRef.current = true
          // Optional haptic — exposes a tactile cue on supporting devices.
          navigator.vibrate?.(15)
          // Fire the initial mousedown at the current finger pos so subsequent
          // moves drag naturally.
          sendMouse('down', 'left', startClientX, startClientY)
        }, LONG_PRESS_MS)
      } else {
        // Trackpad mode: 1-finger drag drives the virtual cursor (which in
        // turn drives the streamed remote cursor via mouse-move).
        trackpadDraggingRef.current = true
        clearLongPress()
        longPressRef.current = setTimeout(() => {
          // Long-press right-click at the cursor's current pos.
          const c = cursorRef.current
          const v = viewport.current
          if (!v) return
          const r = v.getBoundingClientRect()
          sendMouse('down', 'right', r.left + c.x, r.top + c.y)
          sendMouse('up', 'right', r.left + c.x, r.top + c.y)
          navigator.vibrate?.(15)
        }, LONG_PRESS_MS)
      }
    }

    function onPointerMove(e: PointerEvent) {
      const p = pointersRef.current.get(e.pointerId)
      if (!p) return
      if (e.pointerType !== 'touch' && e.pointerType !== 'pen') return
      const dx = e.clientX - p.clientX
      const dy = e.clientY - p.clientY
      p.clientX = e.clientX
      p.clientY = e.clientY
      const totalDx = e.clientX - p.startX
      const totalDy = e.clientY - p.startY
      if (Math.hypot(totalDx, totalDy) > PAN_THRESHOLD_PX) {
        p.moved = true
        clearLongPress()
      }

      if (pointersRef.current.size === 2 && pinchRef.current) {
        const [a, b] = Array.from(pointersRef.current.values())
        const newDist = dist(a, b)
        if (newDist <= 0) return
        const ratio = newDist / pinchRef.current.startDist
        const newScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, pinchRef.current.startScale * ratio))
        // Zoom-at-point: keep the midpoint stationary in the parent's coord
        // space when scale changes.
        const v = viewport.current
        if (!v) return
        const vrect = v.getBoundingClientRect()
        const newMidX = (a.clientX + b.clientX) / 2
        const newMidY = (a.clientY + b.clientY) / 2
        // Midpoint in the layer's parent (viewport) coord space.
        const localMidX = newMidX - vrect.left
        const localMidY = newMidY - vrect.top
        const oldScale = scaleRef.current
        const k = newScale / oldScale
        // newTx = localMidX - (localMidX - oldTx) * k  → keeps the point under
        // the midpoint stationary.
        txRef.current = localMidX - (localMidX - txRef.current) * k
        tyRef.current = localMidY - (localMidY - tyRef.current) * k
        scaleRef.current = newScale
        // Plus centroid translation (2-finger pan).
        const lastMidX = pinchRef.current.midX
        const lastMidY = pinchRef.current.midY
        txRef.current += (newMidX - lastMidX)
        tyRef.current += (newMidY - lastMidY)
        pinchRef.current.midX = newMidX
        pinchRef.current.midY = newMidY
        applyTransform()
        // 2-finger drag also scrolls the wheel in trackpad mode (small,
        // averaged delta — distinct from the pan magnitude).
        if (modeRef.current === 'trackpad') {
          const wdx = Math.round((newMidX - lastMidX) / 8)
          const wdy = Math.round((newMidY - lastMidY) / 8)
          if (wdx !== 0 || wdy !== 0) {
            sendInputRef.current({ t: 'wheel', dx: -wdx, dy: -wdy })
          }
        }
        return
      }

      if (pointersRef.current.size !== 1) return

      if (modeRef.current === 'touch') {
        if (dragArmedRef.current) {
          // User explicitly armed a drag via long-press → forward as
          // mousemove with the button held. The remote sees a real drag.
          sendMouse('move', 'left', e.clientX, e.clientY)
          return
        }
        // Default: 1-finger drag pans the local canvas. NEVER emits a remote
        // mouse event — that's the bug fix vs the legacy useDesktopInput.
        if (p.moved) {
          txRef.current += dx
          tyRef.current += dy
          applyTransform()
        }
      } else {
        // Trackpad mode: 1-finger drag moves the virtual cursor by finger
        // delta with a sensitivity multiplier so fast finger movements
        // traverse the screen without the cursor visibly trailing the
        // finger. Clamped to the PAINTED image bounds (not the viewport)
        // so the cursor can't drift into the letterbox bars and send
        // edge-clamped coords to the remote.
        if (!trackpadDraggingRef.current) return
        const v = viewport.current
        const c = target.current
        if (!v || !c) return
        const vrect = v.getBoundingClientRect()
        const pr = paintedRect(c)
        const minX = pr.left - vrect.left
        const minY = pr.top - vrect.top
        const maxX = minX + pr.width
        const maxY = minY + pr.height
        const cur = cursorRef.current
        const nx = Math.max(minX, Math.min(maxX, cur.x + dx * TRACKPAD_SENSITIVITY))
        const ny = Math.max(minY, Math.min(maxY, cur.y + dy * TRACKPAD_SENSITIVITY))
        setCursor({ x: nx, y: ny, visible: true })
        sendMove(vrect.left + nx, vrect.top + ny)
      }
    }

    function onPointerUp(e: PointerEvent) {
      if (e.pointerType !== 'touch' && e.pointerType !== 'pen') return
      try {
        ;(e.target as Element).releasePointerCapture?.(e.pointerId)
      } catch {
        /* not supported */
      }
      const p = pointersRef.current.get(e.pointerId)
      pointersRef.current.delete(e.pointerId)
      clearLongPress()

      if (pointersRef.current.size === 1) {
        // Just dropped from 2 fingers to 1 — clear pinch state but DON'T
        // treat the remaining finger as a fresh tap.
        pinchRef.current = null
        const [remaining] = Array.from(pointersRef.current.values())
        remaining.moved = true // suppress any spurious tap on lift
        return
      }

      if (pointersRef.current.size === 0 && p) {
        const elapsed = performance.now() - p.startTime
        const moved = p.moved
        const wasPinch = pinchRef.current !== null
        pinchRef.current = null

        if (modeRef.current === 'touch') {
          if (dragArmedRef.current) {
            // End the armed drag — release the left button.
            sendMouse('up', 'left', p.clientX, p.clientY)
            dragArmedRef.current = false
            return
          }
          if (!wasPinch && !moved && elapsed < TAP_MAX_MS) {
            // Tap → click at finger pos.
            sendMouse('down', 'left', p.clientX, p.clientY)
            sendMouse('up', 'left', p.clientX, p.clientY)
          }
          // else: pan-gesture release — local scroll only, no remote event.
        } else {
          // Trackpad mode.
          trackpadDraggingRef.current = false
          if (!wasPinch && !moved && elapsed < TAP_MAX_MS) {
            // Tap → click at the cursor's position.
            const v = viewport.current
            if (!v) return
            const vrect = v.getBoundingClientRect()
            const c = cursorRef.current
            sendMouse('down', 'left', vrect.left + c.x, vrect.top + c.y)
            sendMouse('up', 'left', vrect.left + c.x, vrect.top + c.y)
          }
        }
      }
    }

    function onContextMenu(e: Event) {
      // Block the default Safari long-press context menu — we provide our own.
      e.preventDefault()
    }

    el.addEventListener('pointerdown', onPointerDown)
    el.addEventListener('pointermove', onPointerMove)
    el.addEventListener('pointerup', onPointerUp)
    el.addEventListener('pointercancel', onPointerUp)
    el.addEventListener('contextmenu', onContextMenu)

    return () => {
      el.removeEventListener('pointerdown', onPointerDown)
      el.removeEventListener('pointermove', onPointerMove)
      el.removeEventListener('pointerup', onPointerUp)
      el.removeEventListener('pointercancel', onPointerUp)
      el.removeEventListener('contextmenu', onContextMenu)
      clearLongPress()
      pointersRef.current.clear()
      pinchRef.current = null
    }
  }, [layer, viewport, target, applyTransform, sendMouse, sendMove, disabled])

  // (No ResizeObserver auto-recenter — the mode-change effect above handles
  // initial positioning at canvas centre, and once the user drags the cursor
  // to a new spot we leave it alone. Auto-recentering on every resize would
  // erase finger-driven movements after the on-screen keyboard appears /
  // hides.)

  /** Reset the matrix transform — exposed so the toolbar can offer a
   *  "Reset zoom" button. Not wired by default. */
  const resetZoom = useCallback(() => {
    scaleRef.current = 1
    txRef.current = 0
    tyRef.current = 0
    applyTransform()
  }, [applyTransform])

  return { cursor, resetZoom }
}
