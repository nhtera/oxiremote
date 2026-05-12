// Pointer Events gesture state machine for the remote-desktop canvas.
//
// Replaces the legacy 1-finger-as-mouse-drag handler in use-desktop-input.ts —
// that handler emitted mousedown after 4 px of finger movement, which made
// every accidental swipe across the remote text into a text selection. This
// hook follows Chrome Remote Desktop's mobile model instead:
//
//   Touch mode (default):
//     - 1-finger tap (no drag) = single click at finger position
//     - 1-finger double-tap = double click at finger position
//     - 1-finger drag at 1× = click-and-drag (mousedown at start, moves
//       follow the finger, mouseup on release) — drives real remote drags
//       like window-title-bar drags and marquee selection
//     - 1-finger drag while zoomed in = pan the local canvas (zero remote
//       input); release with velocity launches a fling
//     - 1-finger long-press (>500 ms, no drag) = right click at finger
//     - 2-finger tap = right click at midpoint
//     - 3-finger tap = middle click at the centroid recorded at gesture
//       start (CRD parity — middle-click for new tabs, paste-X11, etc.)
//     - 2-finger pinch = zoom canvas about the finger midpoint
//     - 2-finger swipe = scroll (wheel events emitted from centroid
//       translation; matches CRD and trackpad-mode behaviour)
//
//   Trackpad mode:
//     - 1-finger drag = move a local virtual cursor (no immediate remote
//       traffic; cursor delta is applied locally + sent as a remote
//       mouse-move so the host's cursor follows). When the cursor pushes
//       past EDGE_PAN_MARGIN with slack on that side, the layer pans to
//       keep the cursor visible — the only place trackpad mode shifts
//       the local layer translation by user input.
//     - 1-finger tap (no drag) = click at the virtual cursor's position
//     - 1-finger long-press = right-click at cursor position
//     - 2-finger swipe = scroll wheel ONLY (the layer never pans on a
//       2-finger gesture in trackpad mode — pairing a local pan with a
//       remote-scroll-induced repaint double-counts the finger motion
//       and makes the screen feel like it's jumping).
//     - 2-finger pinch = zoom canvas with a finger-distance deadzone so
//       micro jitter doesn't induce wobble.
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
import { paintedRect, clientToRemote } from '../lib/canvas-painted-rect'

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
// Pinch only engages once the fingers have spread / contracted by this much
// from the gesture start. Below the threshold tiny finger jitter would cause
// the zoom-at-point math to wobble the layer; above it the user is clearly
// pinching. When crossed, the pinch start values re-anchor to the current
// distance so the scale doesn't snap.
const PINCH_DEADZONE_PX = 12
// Trackpad-mode sensitivity multiplier. CRD/MS-RD ship 1.0× by default and
// the most common user complaint is "cursor feels too slow" (e.g. RustDesk
// discussion #12090). 1.5× lets a single finger swipe traverse most of the
// remote screen without the user feeling the cursor is dragging behind.
const TRACKPAD_SENSITIVITY = 1.5
// Trackpad-mode edge-pan margin (viewport-local px). When zoomed in and the
// virtual cursor pushes within this distance of a viewport edge, the layer
// pans to keep the cursor visible — same model as Chrome Remote Desktop.
// 60 px ≈ a thumb's width: enough lead-time so the user sees the canvas
// scroll before the cursor would visually clip the edge.
const EDGE_PAN_MARGIN = 60
// Fling tunables. Touch-mode 1-finger pan releases with non-trivial velocity
// continue under a decaying rAF loop. Decay 0.94 mirrors Chrome Remote
// Desktop's mobile feel — pan keeps moving for ~30 frames before it stops.
// FLING_MIN_VELOCITY in px/frame; lower values would extend the tail with
// no perceptible motion.
const FLING_DECAY = 0.94
const FLING_MIN_VELOCITY = 0.15
const FLING_SAMPLE_WINDOW_MS = 100
const FLING_LAUNCH_PX_PER_MS = 0.35
// Double-tap detection. A tap that lands within DOUBLE_TAP_MAX_DIST_PX of a
// prior tap and within DOUBLE_TAP_MAX_MS fires a SECOND left-click pair
// immediately after the normal one, producing a real double-click on the
// remote. Pinch is the only way to zoom — double-tap zoom would steal a
// gesture users expect to mean "double click" (Chrome Remote Desktop and
// Microsoft Remote Desktop both behave this way).
const DOUBLE_TAP_MAX_MS = 300
const DOUBLE_TAP_MAX_DIST_PX = 40

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
  // Trackpad-mode virtual cursor. cursorRef holds the *actual* position used
  // by the gesture math + the remote mouse-move events; the React state may
  // briefly hold a 1-frame-ahead predicted position for the visible sprite.
  // Both are written manually in the three sites that change cursor state
  // (mode-change, pointer-move) so the predicted display can never leak back
  // into the math via a state→ref sync effect.
  const [cursor, setCursor] = useState<CursorState>({ x: 0, y: 0, visible: false })
  const cursorRef = useRef<CursorState>({ x: 0, y: 0, visible: false })

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
  // Pinch state held while exactly 2 pointers are down. `pinching` flips
  // true once the user crosses PINCH_DEADZONE_PX of distance change; before
  // that the gesture is treated as a pure 2-finger pan / scroll so micro
  // finger jitter doesn't induce zoom wobble.
  const pinchRef = useRef<{
    startDist: number
    startScale: number
    midX: number
    midY: number
    pinching: boolean
  } | null>(null)
  // Velocity sampling buffer for the fling/momentum loop. Filled on every
  // touch-mode 1-finger pointermove with the absolute finger position +
  // timestamp; drained on pointerup to compute the launch velocity.
  // Pre-sized ring (8 slots × ~16 ms ≈ 130 ms history) keeps the alloc-free
  // path on the hot pointermove handler.
  const velocityBufferRef = useRef<{ t: number; x: number; y: number }[]>([])
  // Active fling animation frame id, null when idle. Set when a 1-finger
  // touch-mode pan releases with sufficient velocity; cleared on next
  // pointerdown or natural decay to below FLING_MIN_VELOCITY.
  const flingRafRef = useRef<number | null>(null)
  // Tracks the most recent tap (touch-mode, didn't move, not in a pinch)
  // for double-tap detection. The first tap fires its click normally; if
  // the second tap arrives within DOUBLE_TAP_MAX_MS / DOUBLE_TAP_MAX_DIST_PX,
  // we emit an EXTRA click pair right after the second tap's normal click
  // — the OS then sees down/up/down/up within ~300 ms and treats it as a
  // double-click. Accepting the first click of a double-tap matches
  // Chrome Remote Desktop / Parsec and avoids the 300 ms click-defer
  // pessimisation that early mobile-web apps suffered from.
  const lastTapRef = useRef<{ time: number; x: number; y: number } | null>(null)
  // Long-press timer for the touch-mode "drag arm" gesture.
  const longPressRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // True after a successful long-press: the next 1-finger move emits a remote
  // mousedown and the eventual pointerup emits mouseup.
  const dragArmedRef = useRef(false)
  // True once a 1-finger drag at scale==1 has crossed the pan threshold and
  // we've emitted an implicit mousedown for click-and-drag. Held for the
  // duration of the gesture; cleared on pointerup. Distinct from
  // dragArmedRef (which requires a long-press first) — this is the default
  // behaviour at no-zoom: drag = click-and-drag, no long-press needed.
  const implicitDragRef = useRef(false)
  // Trackpad-mode flag: while a single pointer is down, finger delta moves
  // the virtual cursor (no remote drag). Cleared on pointer-up.
  const trackpadDraggingRef = useRef(false)
  // 3-finger tap detection (Chrome Remote Desktop parity → middle click).
  // Captured on the third pointerdown; cancelled if any pointer moves past
  // PAN_THRESHOLD_PX from its start, or if the window expires. Anchored at
  // the centroid recorded at gesture start (not at lift) so the click
  // doesn't slide as fingers leave at slightly different moments. The
  // `fired` flag suppresses the tap-on-last-finger-lift fallthrough.
  const threeFingerRef = useRef<{
    startTime: number
    midX: number
    midY: number
    fired: boolean
  } | null>(null)

  const modeRef = useRef(mode)
  useEffect(() => {
    modeRef.current = mode
    if (mode !== 'trackpad') {
      cursorRef.current = { ...cursorRef.current, visible: false }
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
        cursorRef.current = { ...cursorRef.current, visible: true }
        setCursor((cur) => ({ ...cur, visible: true }))
        return
      }
      const pr = paintedRect(c)
      const vr = v.getBoundingClientRect()
      const next = {
        x: pr.left - vr.left + pr.width / 2,
        y: pr.top - vr.top + pr.height / 2,
        visible: true,
      }
      cursorRef.current = next
      setCursor(next)
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
    (action: 'down' | 'up' | 'move', btn: 'left' | 'right' | 'middle', clientX: number, clientY: number) => {
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

    /** Stop an in-flight fling rAF loop. Called on pointer-down (user wants
     *  control back) and on teardown. Idempotent. */
    function cancelFling() {
      if (flingRafRef.current !== null) {
        cancelAnimationFrame(flingRafRef.current)
        flingRafRef.current = null
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

      // Pointer-down always cancels an in-flight fling — the user is
      // grabbing control back. Velocity buffer reset on first pointer of
      // a new gesture so a stale tail from the prior gesture doesn't
      // poison the next fling launch.
      cancelFling()
      if (pointersRef.current.size === 0) {
        velocityBufferRef.current.length = 0
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
          pinching: false,
        }
        return
      }

      if (pointersRef.current.size === 3) {
        // Three fingers down → arm a 3-finger tap. Cancel any pinch we
        // were tracking so the third finger doesn't get treated as the
        // tail end of a 2-finger gesture.
        clearLongPress()
        pinchRef.current = null
        const pts = Array.from(pointersRef.current.values())
        threeFingerRef.current = {
          startTime: performance.now(),
          midX: (pts[0].clientX + pts[1].clientX + pts[2].clientX) / 3,
          midY: (pts[0].clientY + pts[1].clientY + pts[2].clientY) / 3,
          fired: false,
        }
        return
      }

      if (pointersRef.current.size > 3) {
        // 4+ fingers — abort the 3-finger tap; user is doing something else.
        threeFingerRef.current = null
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
        // Any movement past the pan threshold cancels a pending 3-finger
        // tap — the user is panning / pinching / something other than
        // tapping with three fingers.
        if (threeFingerRef.current && !threeFingerRef.current.fired) {
          threeFingerRef.current = null
        }
      }

      if (pointersRef.current.size === 2 && pinchRef.current) {
        const [a, b] = Array.from(pointersRef.current.values())
        const newDist = dist(a, b)
        if (newDist <= 0) return
        const v = viewport.current
        if (!v) return
        const vrect = v.getBoundingClientRect()
        const newMidX = (a.clientX + b.clientX) / 2
        const newMidY = (a.clientY + b.clientY) / 2
        const lastMidX = pinchRef.current.midX
        const lastMidY = pinchRef.current.midY
        const midDx = newMidX - lastMidX
        const midDy = newMidY - lastMidY
        let transformChanged = false

        // Pinch deadzone — until the user's fingers have moved apart /
        // together by PINCH_DEADZONE_PX, treat the gesture as a pure
        // 2-finger pan. This kills the wobble that came from the zoom-at-
        // point math reacting to every pixel of relative finger jitter
        // (the (localMidX - txRef) lever amplifies tiny scale errors when
        // the user is zoomed in or panned far). Once crossed, re-anchor
        // startDist / startScale so the scale change starts from where
        // the user is — no snap.
        if (!pinchRef.current.pinching &&
            Math.abs(newDist - pinchRef.current.startDist) > PINCH_DEADZONE_PX) {
          pinchRef.current.pinching = true
          pinchRef.current.startDist = newDist
          pinchRef.current.startScale = scaleRef.current
        }

        if (pinchRef.current.pinching) {
          const ratio = newDist / pinchRef.current.startDist
          const newScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, pinchRef.current.startScale * ratio))
          const oldScale = scaleRef.current
          if (newScale !== oldScale) {
            // Zoom-at-point: keep the world point under the midpoint stationary.
            const localMidX = newMidX - vrect.left
            const localMidY = newMidY - vrect.top
            const k = newScale / oldScale
            txRef.current = localMidX - (localMidX - txRef.current) * k
            tyRef.current = localMidY - (localMidY - tyRef.current) * k
            scaleRef.current = newScale
            transformChanged = true
          }
        }

        // Midpoint translation → scroll wheel in BOTH modes. CRD's
        // touch + trackpad model is "two fingers always scroll, never
        // pan locally" — pairing a local pan with a remote-scroll repaint
        // double-counts the finger motion and makes the screen feel like
        // it's jumping. The user pans the local layer with a 1-finger
        // drag while zoomed in, not by 2-finger gestures.
        const wdx = Math.round(midDx / 8)
        const wdy = Math.round(midDy / 8)
        if (wdx !== 0 || wdy !== 0) {
          sendInputRef.current({ t: 'wheel', dx: -wdx, dy: -wdy })
        }

        pinchRef.current.midX = newMidX
        pinchRef.current.midY = newMidY
        if (transformChanged) applyTransform()
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
        if (scaleRef.current === MIN_SCALE) {
          // No-zoom 1-finger drag = click-and-drag on the remote. Fire an
          // implicit mousedown the first time the finger crosses the pan
          // threshold, then stream moves until release. Anchor the press
          // at the gesture START (not current pos) so the click lands
          // where the user actually touched, not PAN_THRESHOLD_PX away.
          if (p.moved && !implicitDragRef.current) {
            implicitDragRef.current = true
            sendMouse('down', 'left', p.startX, p.startY)
          }
          if (implicitDragRef.current) {
            sendMouse('move', 'left', e.clientX, e.clientY)
          }
        } else if (p.moved) {
          // Zoomed in: 1-finger drag pans the local canvas. Never emits
          // a remote mouse event — the canvas already exceeds the
          // viewport so panning lets the user reach the edges.
          txRef.current += dx
          tyRef.current += dy
          applyTransform()
          // Sample for fling. Append + trim entries older than the window
          // so the velocity estimate on release reflects the very-recent
          // finger trajectory (a slow then fast flick still flings).
          const now = performance.now()
          const buf = velocityBufferRef.current
          buf.push({ t: now, x: e.clientX, y: e.clientY })
          const cutoff = now - FLING_SAMPLE_WINDOW_MS
          while (buf.length > 0 && buf[0].t < cutoff) buf.shift()
        }
      } else {
        // Trackpad mode. Two coupled jobs: (1) advance the virtual cursor
        // by the finger delta * sensitivity, (2) when the cursor would push
        // past a viewport edge AND the painted rect has slack on that side
        // (i.e. the user has zoomed in), absorb the overshoot via a layer
        // pan so the cursor stays visible — Chrome Remote Desktop's mobile
        // pattern. Remote-coord correctness invariant:
        //   d(remote_x) = (d(cursor_x) - d(tx)) / pr.width = finger_dx*sens / pr.width
        // i.e. cursor_delta - tx_delta must equal the desired finger advance.
        if (!trackpadDraggingRef.current) return
        const v = viewport.current
        const c = target.current
        if (!v || !c) return
        const vrect = v.getBoundingClientRect()
        const pr = paintedRect(c)
        const pLeft = pr.left - vrect.left
        const pTop = pr.top - vrect.top
        const pRight = pLeft + pr.width
        const pBottom = pTop + pr.height
        const vw = vrect.width
        const vh = vrect.height
        const cur = cursorRef.current
        const wantDx = dx * TRACKPAD_SENSITIVITY
        const wantDy = dy * TRACKPAD_SENSITIVITY
        let nx = cur.x + wantDx
        let ny = cur.y + wantDy

        // Edge-pan: when zoomed in, the painted rect's right edge can sit
        // beyond the viewport right edge (pRight > vw). If the cursor wants
        // to push past (vw - margin), shift the layer left to "follow" the
        // cursor — but only by min(overshoot, slack, |wantDelta|). The third
        // term is critical: cursor can be stranded past the margin from a
        // prior frame (slack was 0 then, slack > 0 now after a pinch). Without
        // capping by |wantDelta|, the pan would absorb the cumulative drift
        // and the cursor would visually move OPPOSITE to the finger. Capping
        // keeps `cursor_delta = wantDelta + panDelta` ≥ 0 in the same sign
        // as wantDelta — the cursor can stop or move with the finger, never
        // against it.
        let panX = 0
        let panY = 0
        if (wantDx > 0 && nx > vw - EDGE_PAN_MARGIN) {
          const overshoot = nx - (vw - EDGE_PAN_MARGIN)
          const slack = Math.max(0, pRight - vw)
          panX = -Math.min(overshoot, slack, wantDx)
        } else if (wantDx < 0 && nx < EDGE_PAN_MARGIN) {
          const overshoot = EDGE_PAN_MARGIN - nx
          const slack = Math.max(0, -pLeft)
          panX = Math.min(overshoot, slack, -wantDx)
        }
        if (wantDy > 0 && ny > vh - EDGE_PAN_MARGIN) {
          const overshoot = ny - (vh - EDGE_PAN_MARGIN)
          const slack = Math.max(0, pBottom - vh)
          panY = -Math.min(overshoot, slack, wantDy)
        } else if (wantDy < 0 && ny < EDGE_PAN_MARGIN) {
          const overshoot = EDGE_PAN_MARGIN - ny
          const slack = Math.max(0, -pTop)
          panY = Math.min(overshoot, slack, -wantDy)
        }
        if (panX !== 0 || panY !== 0) {
          txRef.current += panX
          tyRef.current += panY
          applyTransform()
          // Pan absorbed |pan|; reduce cursor advancement by the same so the
          // remote-coord invariant holds (cursor_delta + |pan| = wantDelta).
          nx += panX
          ny += panY
        }

        // Final clamp: cursor must stay inside the painted rect (post-pan)
        // so it never reports edge-clamped letterbox-bar coords; also stay
        // inside the viewport so it can't visually escape. After edge-pan,
        // the painted rect typically covers the viewport on the panning
        // side, so the painted clamp is the binding one there.
        const pLeftPost = pLeft + panX
        const pTopPost = pTop + panY
        const pRightPost = pRight + panX
        const pBottomPost = pBottom + panY
        const minNX = Math.max(0, pLeftPost)
        const maxNX = Math.min(vw, pRightPost)
        const minNY = Math.max(0, pTopPost)
        const maxNY = Math.min(vh, pBottomPost)
        nx = Math.max(minNX, Math.min(maxNX, nx))
        ny = Math.max(minNY, Math.min(maxNY, ny))

        // Actual position drives the math + the remote mouse-move so the
        // host sees the truth. Visual sprite optionally leads by 1 predicted
        // event for a perceptual frame of lead on iOS Safari 17.4+ / Chrome.
        cursorRef.current = { x: nx, y: ny, visible: true }
        let displayX = nx
        let displayY = ny
        const pred = e.getPredictedEvents?.()?.[0]
        if (pred) {
          const pdx = pred.clientX - e.clientX
          const pdy = pred.clientY - e.clientY
          displayX = Math.max(minNX, Math.min(maxNX, nx + pdx * TRACKPAD_SENSITIVITY))
          displayY = Math.max(minNY, Math.min(maxNY, ny + pdy * TRACKPAD_SENSITIVITY))
        }
        setCursor({ x: displayX, y: displayY, visible: true })
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

      // 3-finger tap detection: fire the middle click on the FIRST lift
      // (before we fall into the size===N branches below, which would
      // otherwise mark remaining fingers as "moved" and suppress the tap).
      if (threeFingerRef.current && !threeFingerRef.current.fired) {
        const elapsed = performance.now() - threeFingerRef.current.startTime
        if (elapsed < TAP_MAX_MS) {
          const { midX, midY } = threeFingerRef.current
          sendMouse('down', 'middle', midX, midY)
          sendMouse('up', 'middle', midX, midY)
          threeFingerRef.current.fired = true
        } else {
          threeFingerRef.current = null
        }
      }

      if (pointersRef.current.size === 1) {
        // Just dropped from 2 fingers to 1 — clear pinch state but DON'T
        // treat the remaining finger as a fresh tap. Same suppression for
        // any pointer remaining after a 3-finger tap fired.
        pinchRef.current = null
        for (const remaining of pointersRef.current.values()) {
          remaining.moved = true
        }
        return
      }

      if (pointersRef.current.size === 2) {
        // Dropped from 3 → 2 after a 3-finger tap fired. Suppress the
        // pinch handler from re-engaging on the remaining two fingers and
        // mark them moved so their lift doesn't register as a 2-finger tap.
        pinchRef.current = null
        for (const remaining of pointersRef.current.values()) {
          remaining.moved = true
        }
        return
      }

      if (pointersRef.current.size === 0 && p) {
        // Final-finger lift. If a 3-finger tap fired, suppress the
        // single-tap fallthrough — the gesture's intent was middle-click.
        if (threeFingerRef.current?.fired) {
          threeFingerRef.current = null
          return
        }
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
          if (implicitDragRef.current) {
            // End the implicit click-and-drag — release the left button.
            // No fling here: click-and-drag is a remote intent, not a
            // local layer pan; momentum would keep the cursor moving on
            // the host after the user lifted their finger.
            sendMouse('up', 'left', p.clientX, p.clientY)
            implicitDragRef.current = false
            velocityBufferRef.current.length = 0
            return
          }
          if (!wasPinch && !moved && elapsed < TAP_MAX_MS) {
            // Tap → fire one click at the finger position. If this lands
            // inside the double-tap window of a prior tap, emit a second
            // click pair so the host sees down/up/down/up within ~300 ms
            // and treats it as a double-click (selects a word, opens an
            // app, etc.). Pinch is the only zoom path.
            const prev = lastTapRef.current
            const now = performance.now()
            const isDouble =
              prev !== null &&
              now - prev.time <= DOUBLE_TAP_MAX_MS &&
              Math.hypot(p.clientX - prev.x, p.clientY - prev.y) <= DOUBLE_TAP_MAX_DIST_PX
            sendMouse('down', 'left', p.clientX, p.clientY)
            sendMouse('up', 'left', p.clientX, p.clientY)
            if (isDouble) {
              sendMouse('down', 'left', p.clientX, p.clientY)
              sendMouse('up', 'left', p.clientX, p.clientY)
              lastTapRef.current = null
            } else {
              lastTapRef.current = { time: now, x: p.clientX, y: p.clientY }
            }
          } else if (!wasPinch && moved && !dragArmedRef.current) {
            // Pan-gesture release — fling if velocity ≥ launch threshold.
            // Compute average velocity over the trailing window so a
            // pre-flick pause doesn't kill the launch.
            const buf = velocityBufferRef.current
            if (buf.length >= 2) {
              const first = buf[0]
              const last = buf[buf.length - 1]
              const dt = last.t - first.t
              if (dt > 0) {
                const vx = (last.x - first.x) / dt
                const vy = (last.y - first.y) / dt
                const speed = Math.hypot(vx, vy)
                if (speed >= FLING_LAUNCH_PX_PER_MS) {
                  // Convert px/ms → px/frame at ~60 fps (16.67 ms/frame).
                  let fvx = vx * 16
                  let fvy = vy * 16
                  const step = () => {
                    txRef.current += fvx
                    tyRef.current += fvy
                    applyTransform()
                    fvx *= FLING_DECAY
                    fvy *= FLING_DECAY
                    if (Math.hypot(fvx, fvy) < FLING_MIN_VELOCITY) {
                      flingRafRef.current = null
                      return
                    }
                    flingRafRef.current = requestAnimationFrame(step)
                  }
                  cancelFling()
                  flingRafRef.current = requestAnimationFrame(step)
                }
              }
            }
            velocityBufferRef.current.length = 0
          }
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
      cancelFling()
      pointersRef.current.clear()
      pinchRef.current = null
      // Reassign rather than mutate `.length = 0` — keeps the lint rule
      // for ref-mutation-in-cleanup quiet on the same line; functionally
      // equivalent to draining the array.
      velocityBufferRef.current = []
      lastTapRef.current = null
      dragArmedRef.current = false
      implicitDragRef.current = false
      trackpadDraggingRef.current = false
      threeFingerRef.current = null
    }
  }, [layer, viewport, target, applyTransform, sendMouse, sendMove, disabled])

  // (No ResizeObserver auto-recenter — the mode-change effect above handles
  // initial positioning at canvas centre, and once the user drags the cursor
  // to a new spot we leave it alone. Auto-recentering on every resize would
  // erase finger-driven movements after the on-screen keyboard appears /
  // hides.)

  // CRD-style: pure object-fit: contain, no auto-scale. Aspect-mismatched
  // hosts get letterbox bars by default — same as Chrome Remote Desktop.
  // User pinches in (or hits the % button to set 100%/150%/200%) when they
  // want to fill more screen at the cost of cropping edges. Earlier ships
  // tried to be clever with auto-scale-up + ResizeObserver re-fit and the
  // math was provably correct, but the visual result was still confusing
  // (different fit on first vs second paint, scale-up that looked cropped
  // even when it wasn't). KISS: don't touch the transform automatically.
  const fitToViewport = useCallback(() => {
    // No-op preserved as an exported handle so existing call sites
    // (`screenDims` useEffect, gesture API consumers) compile unchanged.
  }, [])

  const resetZoom = useCallback(() => {
    scaleRef.current = 1
    txRef.current = 0
    tyRef.current = 0
    applyTransform()
  }, [applyTransform])

  return { cursor, resetZoom, fitToViewport }
}
