// Global physical-keyboard capture for the desktop session.
//
// While the session is connected and no form control / button is focused,
// every keydown / keyup is forwarded to the host. This is the standard
// VNC / RDP behavior — clicking the canvas hands the keyboard over to
// the remote machine until the user clicks elsewhere.
//
// Two dispatch paths share the existing wire format:
//
//   - **Printable, no Ctrl/Alt/Meta** → `{t:'text', s: e.key}`. The agent
//     calls `enigo.text()` which is Unicode-safe and locale-independent,
//     so accented chars, punctuation, and Shift-produced glyphs (`!`, `@`)
//     land verbatim on hosts of any keyboard layout. Same path the
//     mobile composer uses.
//
//   - **Special keys & modifier combos** → `{t:'key', code, action, mods}`.
//     `code` is `KeyboardEvent.code` (DOM-canonical: `KeyA`, `ArrowUp`,
//     `ShiftLeft`…), which the agent maps via `dom_code_to_key`. Modifier
//     state on the event is forwarded so combos like Ctrl+S round-trip
//     correctly.
//
// Capture skips when:
//   - The active element is an INPUT / TEXTAREA / SELECT / contentEditable.
//     Lets users still type in our settings dialogs and the mobile composer.
//   - The active element is a BUTTON or has `role="button"`. Lets Enter /
//     Space activate focused buttons (modal confirm, sidebar toggles).
//
// Some browser-level shortcuts (Cmd+T, Cmd+W, Cmd+L) the browser swallows
// before our handler runs — those will not be forwarded. Users use the
// sticky-modifier buttons in the sidebar for OS-level keys like
// Ctrl+Alt+Del that the browser hides.

import { useEffect, useRef } from 'react'
import type { DesktopInputEvent } from './use-desktop-session'

export function useDesktopKeyboardCapture(
  enabled: boolean,
  sendInput: (ev: DesktopInputEvent) => void,
) {
  const sendRef = useRef(sendInput)
  useEffect(() => {
    sendRef.current = sendInput
  }, [sendInput])

  useEffect(() => {
    if (!enabled) return

    function shouldCapture(): boolean {
      const el = document.activeElement
      if (!el || el === document.body) return true
      const tag = el.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return false
      if (tag === 'BUTTON') return false
      if ((el as HTMLElement).isContentEditable) return false
      const role = el.getAttribute('role')
      if (role === 'button' || role === 'menuitem' || role === 'option') return false
      return true
    }

    function isPrintable(e: KeyboardEvent): boolean {
      // Single-grapheme `key` values are printable: letters, digits,
      // punctuation, accented chars. Multi-char names like "ArrowUp" or
      // "Backspace" are special keys.
      return e.key.length === 1
    }

    function onKeyDown(e: KeyboardEvent) {
      if (e.isComposing || e.key === 'Process' || e.key === 'Dead') return
      if (!shouldCapture()) return

      const ctrl = e.ctrlKey
      const alt = e.altKey
      const shift = e.shiftKey
      const meta = e.metaKey

      // Printable + no Ctrl/Alt/Meta → text path (Unicode-safe).
      // Shift-only is intentionally text too — `e.key` already reflects the
      // shifted glyph (`!` for Shift+1) so we don't need to round-trip
      // through Shift+Digit1 on the host.
      if (isPrintable(e) && !ctrl && !alt && !meta) {
        e.preventDefault()
        sendRef.current({ t: 'text', s: e.key })
        return
      }

      // Special key or modifier combo. For single-char keys (Ctrl+A,
      // Cmd+S, etc.) we send `e.key` rather than `e.code` so the keystroke
      // matches the user's *typed* character on non-QWERTY layouts —
      // AZERTY's "A" key produces e.key='a' but e.code='KeyQ'; sending
      // KeyQ would route Ctrl+A to Ctrl+Q on the host. The agent's
      // dom_code_to_key falls back to `Key::Unicode(c)` for single-char
      // codes, so 'a' lands correctly. Multi-char names (Backspace,
      // ArrowUp, ShiftLeft, F1) keep using `e.code` since those are
      // layout-independent.
      const code = e.key.length === 1 ? e.key : e.code || e.key
      e.preventDefault()
      sendRef.current({
        t: 'key',
        code,
        action: 'down',
        ctrl,
        alt,
        shift,
        meta,
      })
    }

    function onKeyUp(e: KeyboardEvent) {
      if (e.isComposing) return
      if (!shouldCapture()) return

      const ctrl = e.ctrlKey
      const alt = e.altKey
      const shift = e.shiftKey
      const meta = e.metaKey

      // Printable text was finalised on keydown via the text path; sending
      // a key-up here would tell the host to release a layout-derived
      // keycode and could double-fire on some platforms. Just suppress
      // the local default (some browsers act on keyup) and skip.
      if (isPrintable(e) && !ctrl && !alt && !meta) {
        e.preventDefault()
        return
      }

      // Same layout-aware code selection as keydown.
      const code = e.key.length === 1 ? e.key : e.code || e.key
      e.preventDefault()
      sendRef.current({
        t: 'key',
        code,
        action: 'up',
        ctrl,
        alt,
        shift,
        meta,
      })
    }

    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
    }
  }, [enabled])
}
