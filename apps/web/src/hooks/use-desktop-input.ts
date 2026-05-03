// Touch + Trackpad event handlers for the remote desktop canvas.
// Touch mode: tap → click, two-finger-tap → right-click, long-press → right-click,
//             two-finger swipe → scroll, drag → drag.
// Trackpad mode: mouse move → cursor move, click → click, two-finger scroll → scroll.
//
// Also exports `sendTextBatch`: dispatches a string as a throttled sequence of
// key events (~30 ms/char) so input ordering is preserved on slow tunnels.

import { type RefObject, useCallback, useEffect, useRef } from 'react'
import type { DesktopInputEvent } from './use-desktop-session'

export type InputMode = 'touch' | 'trackpad'

// Character → key code mapping for printable ASCII.
// Keys outside this map are sent as-is using the character itself.
function charToCode(ch: string): string {
  if (ch === ' ') return 'Space'
  if (ch === '\n' || ch === '\r') return 'Enter'
  if (ch === '\t') return 'Tab'
  const upper = ch.toUpperCase()
  // A–Z
  if (/^[A-Z]$/.test(upper)) return `Key${upper}`
  // 0–9
  if (/^[0-9]$/.test(ch)) return `Digit${ch}`
  // Punctuation: use the raw character; the agent maps code → char.
  return ch
}

// Shift-required characters on a US QWERTY layout.
const SHIFT_CHARS = new Set('~!@#$%^&*()_+{}|:"<>?ABCDEFGHIJKLMNOPQRSTUVWXYZ')

/**
 * Sends `text` to the remote machine as a sequence of key-down/key-up events.
 * Throttled at `delayMs` per character (default 30 ms) so the agent's ordered
 * ctrl DataChannel never reorders events under network jitter.
 * Returns a cancel function that stops dispatching mid-sequence.
 */
export function sendTextBatch(
  text: string,
  sendInput: (ev: DesktopInputEvent) => void,
  delayMs = 30,
): () => void {
  let cancelled = false
  let idx = 0

  function sendNext() {
    if (cancelled || idx >= text.length) return
    const ch = text[idx++]
    const code = charToCode(ch)
    const shift = SHIFT_CHARS.has(ch)
    const ev: DesktopInputEvent = { t: 'key', code, action: 'down', ctrl: false, alt: false, shift, meta: false }
    sendInput(ev)
    sendInput({ ...ev, action: 'up' })
    window.setTimeout(sendNext, delayMs)
  }

  // Kick off the first character immediately (no leading delay).
  sendNext()

  return () => { cancelled = true }
}

/**
 * Hook that returns a stable `sendTextBatch` bound to the caller's `sendInput`.
 * The returned function signature matches the phase spec: `(text: string) => void`.
 * Internally it starts the async dispatch and discards the cancel handle —
 * callers that need to abort (e.g. on unmount) should call the raw export.
 */
export function useSendTextBatch(sendInput: (ev: DesktopInputEvent) => void) {
  const sendInputRef = useRef(sendInput)
  useEffect(() => { sendInputRef.current = sendInput }, [sendInput])

  return useCallback((text: string) => {
    sendTextBatch(text, (ev) => sendInputRef.current(ev))
  }, [])
}

/** Higher-level interaction mode that runs alongside `InputMode`. `rect`
 *  remaps single-finger drag into a marquee selection — pointer-down →
 *  pointer-up still emits the same mouse-down/up events as a regular drag,
 *  but the helper draws an overlay so the user sees what they're selecting. */
export type GestureMode = 'pointer' | 'rect'

interface RectOverlayHandle {
  /** Starts/updates the dashed-overlay for the in-progress rectangle. */
  show: (start: { x: number; y: number }, end: { x: number; y: number }) => void
  /** Hides the overlay. */
  hide: () => void
}

interface Props {
  canvas: RefObject<HTMLCanvasElement | null>
  mode: InputMode
  sendInput: (ev: DesktopInputEvent) => void
  /** Optional gesture-mode toggle. When 'rect', single-finger drag draws the
   *  overlay AND emits a normal mouse-drag to the host. */
  gestureMode?: GestureMode
  /** Optional overlay controller — when supplied, rect mode draws via it. */
  rectOverlay?: RectOverlayHandle
}

const LONG_PRESS_MS = 500
const DRAG_THRESHOLD_PX = 4
// 2-finger double-tap window. 9remote-style: tap two fingers, release, tap
// again with two fingers within 300 ms = double-click at midpoint.
const TWO_FINGER_DTAP_MS = 300
const TWO_FINGER_DTAP_PX = 30

function normX(canvas: HTMLCanvasElement, clientX: number): number {
  const rect = canvas.getBoundingClientRect()
  return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
}

function normY(canvas: HTMLCanvasElement, clientY: number): number {
  const rect = canvas.getBoundingClientRect()
  return Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
}

export function useDesktopInput({
  canvas,
  mode,
  sendInput,
  gestureMode = 'pointer',
  rectOverlay,
}: Props) {
  // Refs for touch state — avoids stale closure issues
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchStartPos = useRef<{ x: number; y: number } | null>(null)
  const isDragging = useRef(false)
  const prevTwoFingerY = useRef<number | null>(null)
  const prevTwoFingerX = useRef<number | null>(null)
  // Most recent 2-finger lift, for double-tap detection. Stored in client
  // coords so the distance threshold is intuitive.
  const lastTwoFingerLift = useRef<{ ts: number; cx: number; cy: number } | null>(null)
  // Most recent 2-finger touch start client coords, used to detect a quick
  // tap (touchstart → touchend without significant movement).
  const twoFingerStart = useRef<{ ts: number; cx: number; cy: number } | null>(null)
  // Pixel start of the in-progress rect (for overlay positioning).
  const rectPxStart = useRef<{ cx: number; cy: number } | null>(null)
  // Read latest gestureMode/overlay inside event handlers without re-binding
  // the listeners (which would lose state).
  const gestureModeRef = useRef(gestureMode)
  const rectOverlayRef = useRef(rectOverlay)
  useEffect(() => { gestureModeRef.current = gestureMode }, [gestureMode])
  useEffect(() => { rectOverlayRef.current = rectOverlay }, [rectOverlay])

  useEffect(() => {
    const el = canvas.current
    if (!el) return

    // ── TOUCH MODE ──────────────────────────────────────────────────────────

    function onTouchStart(e: TouchEvent) {
      e.preventDefault()
      const t = e.touches
      isDragging.current = false

      if (t.length === 1) {
        const x = normX(el!, t[0].clientX)
        const y = normY(el!, t[0].clientY)
        touchStartPos.current = { x, y }
        // Track pixel start for the optional rect overlay.
        if (gestureModeRef.current === 'rect') {
          rectPxStart.current = { cx: t[0].clientX, cy: t[0].clientY }
        }

        // Send mouse move first so cursor is at tap position
        sendInput({ t: 'mouse', action: 'move', x, y })

        // Long-press → right-click after LONG_PRESS_MS with no movement
        longPressTimer.current = setTimeout(() => {
          sendInput({ t: 'mouse', action: 'down', btn: 'right', x, y })
          sendInput({ t: 'mouse', action: 'up', btn: 'right', x, y })
        }, LONG_PRESS_MS)
      } else if (t.length === 2) {
        // Cancel single-touch long press
        if (longPressTimer.current) clearTimeout(longPressTimer.current)
        const avgY = (t[0].clientY + t[1].clientY) / 2
        const avgX = (t[0].clientX + t[1].clientX) / 2
        prevTwoFingerY.current = avgY
        prevTwoFingerX.current = avgX
        twoFingerStart.current = { ts: performance.now(), cx: avgX, cy: avgY }
      }
    }

    function onTouchMove(e: TouchEvent) {
      e.preventDefault()
      const t = e.touches

      if (t.length === 1 && touchStartPos.current) {
        const x = normX(el!, t[0].clientX)
        const y = normY(el!, t[0].clientY)
        const dx = Math.abs(t[0].clientX - (touchStartPos.current.x * el!.getBoundingClientRect().width + el!.getBoundingClientRect().left))
        const dy = Math.abs(t[0].clientY - (touchStartPos.current.y * el!.getBoundingClientRect().height + el!.getBoundingClientRect().top))

        if (!isDragging.current && (dx > DRAG_THRESHOLD_PX || dy > DRAG_THRESHOLD_PX)) {
          // Cancel long-press — finger moved
          if (longPressTimer.current) clearTimeout(longPressTimer.current)
          isDragging.current = true
          sendInput({ t: 'mouse', action: 'down', btn: 'left', x: touchStartPos.current.x, y: touchStartPos.current.y })
        }

        sendInput({ t: 'mouse', action: 'move', x, y })

        // Rect-mode overlay: redraw the dashed rectangle from the start
        // position to the current finger. The overlay is purely visual; the
        // server still receives the same mouse-down / move / up triple as
        // a regular drag.
        if (gestureModeRef.current === 'rect' && rectPxStart.current && rectOverlayRef.current) {
          rectOverlayRef.current.show(
            { x: rectPxStart.current.cx, y: rectPxStart.current.cy },
            { x: t[0].clientX, y: t[0].clientY },
          )
        }
      } else if (t.length === 2) {
        const avgY = (t[0].clientY + t[1].clientY) / 2
        const avgX = (t[0].clientX + t[1].clientX) / 2

        if (prevTwoFingerY.current !== null && prevTwoFingerX.current !== null) {
          const dy = Math.round((prevTwoFingerY.current - avgY) / 8)
          const dx = Math.round((prevTwoFingerX.current - avgX) / 8)
          if (dy !== 0 || dx !== 0) {
            sendInput({ t: 'wheel', dx, dy })
          }
        }
        prevTwoFingerY.current = avgY
        prevTwoFingerX.current = avgX
      }
    }

    function onTouchEnd(e: TouchEvent) {
      e.preventDefault()
      if (longPressTimer.current) clearTimeout(longPressTimer.current)

      const changedTouches = e.changedTouches
      const remaining = e.touches

      if (remaining.length === 0 && changedTouches.length >= 1) {
        const x = normX(el!, changedTouches[0].clientX)
        const y = normY(el!, changedTouches[0].clientY)

        if (isDragging.current) {
          // End drag
          sendInput({ t: 'mouse', action: 'up', btn: 'left', x, y })
          isDragging.current = false
          // Rect overlay clears on release; the host already received the
          // drag events so the selection is in place.
          if (gestureModeRef.current === 'rect' && rectOverlayRef.current) {
            rectOverlayRef.current.hide()
          }
          rectPxStart.current = null
        } else if (changedTouches.length === 1) {
          // Tap → left click
          sendInput({ t: 'mouse', action: 'down', btn: 'left', x, y })
          sendInput({ t: 'mouse', action: 'up', btn: 'left', x, y })
        } else if (changedTouches.length === 2) {
          // Two-finger lift. Decide between a single right-click (the legacy
          // gesture) and a 2-finger double-tap → double-click. The latter
          // requires the two fingers to have landed and lifted within a
          // short window, with low movement.
          const mxPx = (changedTouches[0].clientX + changedTouches[1].clientX) / 2
          const myPx = (changedTouches[0].clientY + changedTouches[1].clientY) / 2
          const start = twoFingerStart.current
          const moved =
            start &&
            (Math.hypot(mxPx - start.cx, myPx - start.cy) > TWO_FINGER_DTAP_PX)
          const last = lastTwoFingerLift.current
          const now = performance.now()
          const isDoubleTap =
            !moved &&
            last &&
            now - last.ts < TWO_FINGER_DTAP_MS &&
            Math.hypot(mxPx - last.cx, myPx - last.cy) < TWO_FINGER_DTAP_PX

          if (isDoubleTap) {
            const mx = normX(el!, mxPx)
            const my = normY(el!, myPx)
            // Two left-clicks back-to-back → double-click on the OS side.
            sendInput({ t: 'mouse', action: 'down', btn: 'left', x: mx, y: my })
            sendInput({ t: 'mouse', action: 'up', btn: 'left', x: mx, y: my })
            sendInput({ t: 'mouse', action: 'down', btn: 'left', x: mx, y: my })
            sendInput({ t: 'mouse', action: 'up', btn: 'left', x: mx, y: my })
            lastTwoFingerLift.current = null
          } else {
            // Two-finger tap → right click at midpoint (legacy behavior).
            const mx = normX(el!, mxPx)
            const my = normY(el!, myPx)
            sendInput({ t: 'mouse', action: 'down', btn: 'right', x: mx, y: my })
            sendInput({ t: 'mouse', action: 'up', btn: 'right', x: mx, y: my })
            // Remember this lift for the next double-tap candidate window.
            lastTwoFingerLift.current = { ts: now, cx: mxPx, cy: myPx }
          }
        }
      }

      if (remaining.length < 2) {
        prevTwoFingerY.current = null
        prevTwoFingerX.current = null
        twoFingerStart.current = null
      }
      if (remaining.length === 0) {
        touchStartPos.current = null
        rectPxStart.current = null
      }
    }

    // ── TRACKPAD (MOUSE) MODE ────────────────────────────────────────────────

    function onMouseMove(e: MouseEvent) {
      const x = normX(el!, e.clientX)
      const y = normY(el!, e.clientY)
      sendInput({ t: 'mouse', action: 'move', x, y })
    }

    function onMouseDown(e: MouseEvent) {
      const x = normX(el!, e.clientX)
      const y = normY(el!, e.clientY)
      const btn = e.button === 2 ? 'right' : e.button === 1 ? 'middle' : 'left'
      sendInput({ t: 'mouse', action: 'down', btn, x, y })
    }

    function onMouseUp(e: MouseEvent) {
      const x = normX(el!, e.clientX)
      const y = normY(el!, e.clientY)
      const btn = e.button === 2 ? 'right' : e.button === 1 ? 'middle' : 'left'
      sendInput({ t: 'mouse', action: 'up', btn, x, y })
    }

    function onWheel(e: WheelEvent) {
      e.preventDefault()
      const dx = Math.round(e.deltaX / 40)
      const dy = Math.round(e.deltaY / 40)
      sendInput({ t: 'wheel', dx, dy })
    }

    function onContextMenu(e: MouseEvent) {
      e.preventDefault()
    }

    // Detect whether the device has a real fine-grained pointer (mouse,
    // trackpad). Touch-only mobile reports `any-pointer: coarse` but not
    // `fine`. Without this gate, iOS synthesises mouse events from finger
    // taps and the trackpad-mode mouse handlers below double-fire on top of
    // useCanvasGestures' tap, sending a click at the finger position that
    // competes with the click at the virtual cursor.
    const hasFinePointer =
      typeof window !== 'undefined' &&
      window.matchMedia?.('(any-pointer: fine)')?.matches === true

    if (mode === 'touch') {
      // Touch handlers only run for the rect-marquee gesture mode now —
      // useCanvasGestures owns default touch input (pinch-zoom, 1-finger
      // pan, virtual cursor). Without this gate both hooks would emit
      // duplicate remote events on every tap.
      if (gestureModeRef.current === 'rect') {
        el.addEventListener('touchstart', onTouchStart, { passive: false })
        el.addEventListener('touchmove', onTouchMove, { passive: false })
        el.addEventListener('touchend', onTouchEnd, { passive: false })
        el.addEventListener('touchcancel', onTouchEnd, { passive: false })
      }
    } else if (hasFinePointer) {
      el.addEventListener('mousemove', onMouseMove)
      el.addEventListener('mousedown', onMouseDown)
      el.addEventListener('mouseup', onMouseUp)
      el.addEventListener('wheel', onWheel, { passive: false })
      el.addEventListener('contextmenu', onContextMenu)
    }

    return () => {
      if (longPressTimer.current) clearTimeout(longPressTimer.current)
      if (mode === 'touch') {
        if (gestureModeRef.current === 'rect') {
          el.removeEventListener('touchstart', onTouchStart)
          el.removeEventListener('touchmove', onTouchMove)
          el.removeEventListener('touchend', onTouchEnd)
          el.removeEventListener('touchcancel', onTouchEnd)
        }
      } else if (hasFinePointer) {
        el.removeEventListener('mousemove', onMouseMove)
        el.removeEventListener('mousedown', onMouseDown)
        el.removeEventListener('mouseup', onMouseUp)
        el.removeEventListener('wheel', onWheel)
        el.removeEventListener('contextmenu', onContextMenu)
      }
    }
  }, [canvas, mode, sendInput, gestureMode])
}
