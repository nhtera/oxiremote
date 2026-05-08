// RVNC-style mobile keyboard accessory. Renders as the last flex-column
// child of the desktop page so the canvas wrap (flex-1 above it) reflows
// to fit the area between the top strip and this bar. The desktop page
// owns the soft-keyboard padding (paddingBottom = keyboardOffset on the
// outer container), which pushes both the canvas wrap AND this bar above
// the keyboard. Net effect: image rides up with the keyboard, composer
// sits on top of the keyboard, no fixed-positioning gymnastics.
//
// Lifecycle:
//   - Parent controls visibility via `open`. When `open` flips true we
//     focus the inner input (rAF-deferred so iOS doesn't choke on the
//     focus during a touch handler) and iOS pops the soft keyboard.
//   - When the user dismisses the keyboard via the OS gesture (swipe
//     down, tap "Done", tap canvas), the visualViewport collapses; we
//     detect the down-edge and call `onClose` so the parent can hide us.
//   - Tap the ▼ button → `onClose` → input blurs → keyboard goes down
//     → bar unmounts.
//
// Typing dispatch is RVNC-style live diff: the typed text stays visible
// in the input field AND every keystroke streams to the remote in real
// time. The user sees what they sent on the field as well as on the
// remote screen.
//
//   - User types → chars land in the input's `value` and stay there.
//   - On `input` event we compute the delta against the previous value:
//       * removed-suffix → dispatch one Backspace per removed char
//       * added-suffix   → dispatch as `{t:'text', s}`
//     This handles append, mid-string edits, autocorrect, and paste
//     uniformly because the remote always mirrors the local field's
//     contents from the diff-point onward.
//   - IME composition (CJK / VI tone marks) pauses dispatch via a flag
//     on `compositionstart`; on `compositionend` we reconcile whatever
//     the IME committed.
//   - Enter sends a key frame and clears the field so the next message
//     starts fresh (matches iOS' "send" enter-key hint UX).
//   - Backspace flows through the natural delete → `input` → diff path,
//     no special handler needed.

import { useEffect, useRef } from 'react'
import type { DesktopInputEvent } from '../hooks/use-desktop-session'

interface Props {
  /** Visibility — when true, mounts the bar and focuses the input. */
  open: boolean
  /** Hide the bar (and dismiss the soft keyboard). */
  onClose: () => void
  /** Dispatch a literal string as one (or chunked) `{t:'text'}` frame. */
  onSend: (text: string) => void
  /** Dispatch a single key event (caller forwards down + up via dispatchKey). */
  onKey: (ev: DesktopInputEvent) => void
  /** Open the full keyboard sheet (modifiers, arrows, F-keys). */
  onOpenSheet: () => void
}

export default function DesktopTextStrip({ open, onClose, onSend, onKey, onOpenSheet }: Props) {
  const inputRef = useRef<HTMLInputElement>(null)
  const composingRef = useRef(false)
  // Last-seen value of the input — diff baseline for the next `input`
  // event. Reset whenever the user finalises a message (Enter / send).
  const prevValueRef = useRef('')
  const keyboardWasUpRef = useRef(false)
  const onCloseRef = useRef(onClose)
  useEffect(() => {
    onCloseRef.current = onClose
  }, [onClose])

  // Focus the input when the bar opens. rAF defer so iOS Safari doesn't
  // choke on focusing inside the same tick as the trigger touch handler.
  // We don't need a `!open` branch — the early return below unmounts the
  // bar so the input goes with it, and the next mount starts fresh.
  useEffect(() => {
    if (!open) return
    const id = requestAnimationFrame(() => {
      inputRef.current?.focus()
    })
    return () => cancelAnimationFrame(id)
  }, [open])

  // Track soft-keyboard via visualViewport — only to detect the down-edge
  // when the user dismisses the keyboard via OS gesture (swipe, "Done",
  // tap canvas). Positioning is no longer needed here: the parent's
  // paddingBottom (= keyboardOffset on the outer flex container) pushes
  // this bar above the keyboard naturally as a flex-column child.
  useEffect(() => {
    if (!open) return
    const vp = window.visualViewport
    if (!vp) return
    function update() {
      if (!vp) return
      const obscured = Math.max(0, window.innerHeight - vp.height - vp.offsetTop)
      // Thresholds are a percentage of the layout viewport so split / mini
      // soft keyboards (Gboard mini, iPad floating) trigger correctly. Up
      // edge: 12% (~100 px on a 6.1" phone, ~50 px on a small Android).
      // Down edge: 4% so we close shortly after the keyboard collapses.
      const upThreshold = window.innerHeight * 0.12
      const downThreshold = window.innerHeight * 0.04
      if (obscured > upThreshold) {
        keyboardWasUpRef.current = true
      } else if (keyboardWasUpRef.current && obscured < downThreshold) {
        keyboardWasUpRef.current = false
        onCloseRef.current()
      }
    }
    vp.addEventListener('resize', update)
    vp.addEventListener('scroll', update)
    update()
    return () => {
      vp.removeEventListener('resize', update)
      vp.removeEventListener('scroll', update)
    }
  }, [open])

  function dispatchKey(code: string) {
    onKey({ t: 'key', code, action: 'down', ctrl: false, alt: false, shift: false, meta: false })
    onKey({ t: 'key', code, action: 'up', ctrl: false, alt: false, shift: false, meta: false })
  }

  // Diff the new field value against the last-seen value and forward the
  // delta. Walks a common prefix; whatever the prev value had after that
  // prefix gets removed via Backspace on the remote (Backspace pops the
  // host's input tail), and whatever the new value has after that prefix
  // gets sent as a literal text frame. Net effect: the remote's typed
  // text always mirrors the local input field's tail from the diff point.
  //
  // Handles append, mid-string edit, paste, autocorrect, and select-all
  // replace uniformly. Cursor position is intentionally ignored —
  // matches RVNC / CRD / noVNC, which all assume "host caret = local
  // caret" by appending and pop-from-tail.
  function reconcile(target: HTMLInputElement) {
    const newValue = target.value
    const prevValue = prevValueRef.current
    if (newValue === prevValue) return
    let common = 0
    const minLen = Math.min(prevValue.length, newValue.length)
    while (common < minLen && prevValue[common] === newValue[common]) {
      common++
    }
    const removed = prevValue.length - common
    const added = newValue.slice(common)
    for (let i = 0; i < removed; i++) {
      dispatchKey('Backspace')
    }
    if (added.length > 0) {
      onSend(added)
    }
    prevValueRef.current = newValue
  }

  function onInput(e: React.SyntheticEvent<HTMLInputElement>) {
    if (composingRef.current) return
    reconcile(e.currentTarget)
  }

  function onCompositionStart() {
    composingRef.current = true
  }
  function onCompositionEnd(e: React.CompositionEvent<HTMLInputElement>) {
    composingRef.current = false
    reconcile(e.currentTarget)
  }

  function clearField() {
    if (inputRef.current) {
      inputRef.current.value = ''
    }
    prevValueRef.current = ''
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (composingRef.current) return
    if (e.key === 'Enter') {
      e.preventDefault()
      dispatchKey('Enter')
      // Reset the field so the next message starts on a clean slate and
      // prevValueRef doesn't grow unbounded.
      clearField()
    }
    // Backspace falls through — iOS deletes a char from the field, the
    // input event fires, and reconcile() forwards the Backspace to the
    // remote.
  }

  function onBackspaceClick() {
    const el = inputRef.current
    if (!el) return
    if (el.value.length > 0) {
      // Pop a char locally and let reconcile forward the Backspace.
      el.value = el.value.slice(0, -1)
      reconcile(el)
    } else {
      // Field already empty — still fire Backspace at the remote so the
      // button works as a remote-only Backspace once the field is dry.
      dispatchKey('Backspace')
    }
    el.focus()
  }
  function onEnterClick() {
    dispatchKey('Enter')
    clearField()
    inputRef.current?.focus()
  }

  if (!open) return null

  return (
    <div
      className="lg:hidden flex items-center gap-1.5 px-2 py-1.5 bg-surface border-t border-border shrink-0"
      role="group"
      aria-label="Send text to remote"
    >
      <ActionButton
        onClick={onOpenSheet}
        title="Open keyboard sheet (modifiers, arrows, F-keys)"
        aria-label="Open keyboard sheet"
      >
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <rect x="1.75" y="4.5" width="12.5" height="7" rx="1.25" />
          <path d="M4 7h.01M6.5 7h.01M9 7h.01M11.5 7h.01M4.5 9.5h7" strokeLinecap="round" />
        </svg>
      </ActionButton>

      <input
        ref={inputRef}
        type="text"
        // Uncontrolled — the field shows the typed text live (RVNC-style)
        // while reconcile() streams the diff to the remote. Cleared only
        // on Enter / send. defaultValue keeps React out of the value loop.
        defaultValue=""
        onInput={onInput}
        onCompositionStart={onCompositionStart}
        onCompositionEnd={onCompositionEnd}
        onKeyDown={onKeyDown}
        placeholder="Send text…"
        aria-label="Text to send to remote machine"
        inputMode="text"
        enterKeyHint="send"
        autoCapitalize="off"
        autoCorrect="off"
        autoComplete="off"
        spellCheck={false}
        className={[
          'flex-1 min-w-0 h-9 px-3 rounded-md',
          'bg-surface-alt border border-border',
          'text-sm text-text-primary placeholder:text-text-muted',
          'focus:outline-none focus:border-[hsl(var(--accent-primary)/0.6)]',
        ].join(' ')}
      />

      <ActionButton
        onClick={onBackspaceClick}
        title="Send Backspace to remote"
        aria-label="Send Backspace key"
      >
        <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M14 3.5H6.5L2 8l4.5 4.5H14a1 1 0 0 0 1-1v-7a1 1 0 0 0-1-1z" />
          <path d="M9 6.5l3 3M12 6.5l-3 3" />
        </svg>
      </ActionButton>

      <ActionButton
        onClick={onEnterClick}
        title="Send Enter to remote"
        aria-label="Send Enter key"
        variant="accent"
      >
        <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M13 4v3a2 2 0 0 1-2 2H3" />
          <path d="M6 6 3 9l3 3" />
        </svg>
      </ActionButton>

      <ActionButton
        onClick={onClose}
        title="Hide keyboard"
        aria-label="Hide keyboard"
      >
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </ActionButton>
    </div>
  )
}

interface ActionButtonProps {
  onClick: () => void
  title: string
  'aria-label': string
  variant?: 'default' | 'accent'
  children: React.ReactNode
}

function ActionButton({ onClick, title, variant = 'default', children, ...rest }: ActionButtonProps) {
  const tone =
    variant === 'accent'
      ? 'bg-[hsl(var(--accent-primary)/0.18)] border-[hsl(var(--accent-primary)/0.45)] text-[hsl(var(--accent-primary))] hover:bg-[hsl(var(--accent-primary)/0.28)]'
      : 'bg-surface-alt border-border text-text-secondary hover:text-text-primary hover:bg-surface-hover'
  return (
    <button
      type="button"
      // Prevent the implicit input blur so iOS Safari keeps the soft
      // keyboard up when the action buttons are tapped.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      title={title}
      className={[
        'shrink-0 inline-flex items-center justify-center w-9 h-9 rounded-md border transition-colors active:scale-95',
        tone,
      ].join(' ')}
      {...rest}
    >
      {children}
    </button>
  )
}
