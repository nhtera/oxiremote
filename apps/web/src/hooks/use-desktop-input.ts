// Touch + Trackpad event handlers for the remote desktop canvas.
// Touch mode: tap → click, two-finger-tap → right-click, long-press → right-click,
//             two-finger swipe → scroll, drag → drag.
// Trackpad mode: mouse move → cursor move, click → click, two-finger scroll → scroll.

import { type RefObject, useEffect, useRef } from 'react'
import type { DesktopInputEvent } from './use-desktop-session'

export type InputMode = 'touch' | 'trackpad'

interface Props {
  canvas: RefObject<HTMLCanvasElement | null>
  mode: InputMode
  sendInput: (ev: DesktopInputEvent) => void
}

const LONG_PRESS_MS = 500
const DRAG_THRESHOLD_PX = 4

function normX(canvas: HTMLCanvasElement, clientX: number): number {
  const rect = canvas.getBoundingClientRect()
  return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
}

function normY(canvas: HTMLCanvasElement, clientY: number): number {
  const rect = canvas.getBoundingClientRect()
  return Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
}

export function useDesktopInput({ canvas, mode, sendInput }: Props) {
  // Refs for touch state — avoids stale closure issues
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchStartPos = useRef<{ x: number; y: number } | null>(null)
  const isDragging = useRef(false)
  const prevTwoFingerY = useRef<number | null>(null)
  const prevTwoFingerX = useRef<number | null>(null)

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
        } else if (changedTouches.length === 1) {
          // Tap → left click
          sendInput({ t: 'mouse', action: 'down', btn: 'left', x, y })
          sendInput({ t: 'mouse', action: 'up', btn: 'left', x, y })
        } else if (changedTouches.length === 2) {
          // Two-finger tap → right click at midpoint
          const mx = normX(el!, (changedTouches[0].clientX + changedTouches[1].clientX) / 2)
          const my = normY(el!, (changedTouches[0].clientY + changedTouches[1].clientY) / 2)
          sendInput({ t: 'mouse', action: 'down', btn: 'right', x: mx, y: my })
          sendInput({ t: 'mouse', action: 'up', btn: 'right', x: mx, y: my })
        }
      }

      if (remaining.length < 2) {
        prevTwoFingerY.current = null
        prevTwoFingerX.current = null
      }
      if (remaining.length === 0) {
        touchStartPos.current = null
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

    if (mode === 'touch') {
      el.addEventListener('touchstart', onTouchStart, { passive: false })
      el.addEventListener('touchmove', onTouchMove, { passive: false })
      el.addEventListener('touchend', onTouchEnd, { passive: false })
      el.addEventListener('touchcancel', onTouchEnd, { passive: false })
    } else {
      el.addEventListener('mousemove', onMouseMove)
      el.addEventListener('mousedown', onMouseDown)
      el.addEventListener('mouseup', onMouseUp)
      el.addEventListener('wheel', onWheel, { passive: false })
      el.addEventListener('contextmenu', onContextMenu)
    }

    return () => {
      if (longPressTimer.current) clearTimeout(longPressTimer.current)
      if (mode === 'touch') {
        el.removeEventListener('touchstart', onTouchStart)
        el.removeEventListener('touchmove', onTouchMove)
        el.removeEventListener('touchend', onTouchEnd)
        el.removeEventListener('touchcancel', onTouchEnd)
      } else {
        el.removeEventListener('mousemove', onMouseMove)
        el.removeEventListener('mousedown', onMouseDown)
        el.removeEventListener('mouseup', onMouseUp)
        el.removeEventListener('wheel', onWheel)
        el.removeEventListener('contextmenu', onContextMenu)
      }
    }
  }, [canvas, mode, sendInput])
}
