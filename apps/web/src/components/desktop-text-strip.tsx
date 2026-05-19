// RVNC-style mobile keyboard accessory. Renders as the last flex-column
// child of the desktop page so the canvas wrap (flex-1 above it) reflows
// to fit the area between the top strip and this bar. The desktop page
// owns the soft-keyboard padding (paddingBottom = keyboardOffset on the
// outer container), which pushes both the canvas wrap AND this bar above
// the keyboard. Net effect: image rides up with the keyboard, composer
// sits on top of the keyboard, no fixed-positioning gymnastics.
//
// Layout (top → bottom):
//   Row 1: sticky modifier toggles (Ctrl/Alt/Shift/Meta) + quick keys
//          (Esc/Tab/arrows) + ⌨↓ peek + … sheet launcher.
//   Row 2: text input + ⌫ + ↵ + ▼ close.
//
// Modifier model — matches DesktopToolbar / DesktopOnscreenKeyboard:
//   - Tap a modifier: toggles it on (filled state).
//   - Modifier stays sticky until consumed by the next non-mod dispatch,
//     then auto-clears so a second key tap is unmodified.
//   - Tap a quick key (Esc, Tab, arrow) while mods are active: dispatches
//     with mod flags, clears mods.
//   - Type a char in the text input while mods are active: intercepts the
//     `input` event, reverts the local field (the char is a command, not
//     text), dispatches `{t:'key', code, action, mods}` for each added
//     char, clears mods. Lets the operator do Ctrl+C, Alt+Tab, Cmd+Q, etc.
//     without leaving the soft keyboard.
//
// Peek (⌨↓): blurs the input so iOS / Android dismiss the soft keyboard
// but the bar stays mounted with modifiers intact. The down-edge close
// detector is silenced while peeked so the strip doesn't auto-close.
// Tap the input to bring the keyboard back; tap ▼ to close everything.
//
// Typing dispatch is RVNC-style live diff (mods OFF path): the typed
// text stays visible in the input field AND every keystroke streams to
// the remote in real time.
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

import { useEffect, useRef, useState } from 'react'
import type { DesktopInputEvent } from '../hooks/use-desktop-session'

type ModKey = 'ctrl' | 'alt' | 'shift' | 'meta'

interface Props {
  /** Visibility — when true, mounts the bar and focuses the input. */
  open: boolean
  /** Hide the bar (and dismiss the soft keyboard). */
  onClose: () => void
  /** Dispatch a literal string as one (or chunked) `{t:'text'}` frame. */
  onSend: (text: string) => void
  /** Dispatch a single key event (caller forwards down + up via dispatchKey). */
  onKey: (ev: DesktopInputEvent) => void
  /** Open the full keyboard sheet (F-keys, Home/End/PgUp/PgDn). */
  onOpenSheet: () => void
}

interface QuickKey {
  label: string
  code: string
  aria: string
}

const QUICK_KEYS: QuickKey[] = [
  { label: 'Esc', code: 'Escape', aria: 'Send Escape' },
  { label: 'Tab', code: 'Tab', aria: 'Send Tab' },
  { label: '←', code: 'ArrowLeft', aria: 'Send Left Arrow' },
  { label: '↑', code: 'ArrowUp', aria: 'Send Up Arrow' },
  { label: '↓', code: 'ArrowDown', aria: 'Send Down Arrow' },
  { label: '→', code: 'ArrowRight', aria: 'Send Right Arrow' },
]

const MOD_KEYS: { label: string; mod: ModKey; aria: string }[] = [
  { label: 'Ctrl', mod: 'ctrl', aria: 'Toggle Ctrl modifier' },
  { label: 'Alt', mod: 'alt', aria: 'Toggle Alt modifier' },
  { label: 'Shift', mod: 'shift', aria: 'Toggle Shift modifier' },
  { label: '⌘', mod: 'meta', aria: 'Toggle Command / Meta modifier' },
]

// Map a single typed char to a DOM `code` value the agent understands.
// Letters → `KeyX`, digits → `DigitN`, everything else → the char itself
// (the agent has a single-char Unicode fallback for punctuation).
function charToCode(ch: string): string {
  if (ch.length !== 1) return ch
  if (/[a-zA-Z]/.test(ch)) return `Key${ch.toUpperCase()}`
  if (/[0-9]/.test(ch)) return `Digit${ch}`
  return ch
}

export default function DesktopTextStrip({ open, onClose, onSend, onKey, onOpenSheet }: Props) {
  const inputRef = useRef<HTMLInputElement>(null)
  const composingRef = useRef(false)
  // Last-seen value of the input — diff baseline for the next `input`
  // event. Reset whenever the user finalises a message (Enter / send) or
  // intercepts a mod-combo (we revert the field to prev value).
  const prevValueRef = useRef('')
  const keyboardWasUpRef = useRef(false)
  // True after the user taps the peek (⌨↓) button. Suppresses the
  // visualViewport down-edge auto-close so the strip stays mounted while
  // the soft keyboard is dismissed. Reset when the input refocuses.
  const peekedRef = useRef(false)
  const onCloseRef = useRef(onClose)
  useEffect(() => {
    onCloseRef.current = onClose
  }, [onClose])

  // Sticky modifiers. React state (not a ref) so the row's visual
  // pressed-state stays in sync — input handlers are recreated each
  // render and capture the latest set.
  const [activeMods, setActiveMods] = useState<Set<ModKey>>(new Set())

  function toggleMod(mod: ModKey) {
    setActiveMods((prev) => {
      const next = new Set(prev)
      if (next.has(mod)) next.delete(mod)
      else next.add(mod)
      return next
    })
  }

  function modsToFlags(mods: Set<ModKey>) {
    return {
      ctrl: mods.has('ctrl'),
      alt: mods.has('alt'),
      shift: mods.has('shift'),
      meta: mods.has('meta'),
    }
  }

  // Focus the input when the bar opens (unless we're in peek mode). rAF
  // defer so iOS Safari doesn't choke on focusing inside the same tick
  // as the trigger touch handler.
  useEffect(() => {
    if (!open) return
    if (peekedRef.current) return
    const id = requestAnimationFrame(() => {
      inputRef.current?.focus()
    })
    return () => cancelAnimationFrame(id)
  }, [open])

  // Track soft-keyboard via visualViewport — only to detect the down-edge
  // when the user dismisses the keyboard via OS gesture (swipe, "Done",
  // tap canvas). Positioning is owned by the parent's paddingBottom.
  // Peek mode silences this detector so the strip survives an explicit
  // dismiss-keyboard tap.
  useEffect(() => {
    if (!open) return
    const vp = window.visualViewport
    if (!vp) return
    function update() {
      if (!vp) return
      const obscured = Math.max(0, window.innerHeight - vp.height - vp.offsetTop)
      const upThreshold = window.innerHeight * 0.12
      const downThreshold = window.innerHeight * 0.04
      if (obscured > upThreshold) {
        keyboardWasUpRef.current = true
      } else if (keyboardWasUpRef.current && obscured < downThreshold) {
        keyboardWasUpRef.current = false
        if (peekedRef.current) return
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

  function dispatchKeyEvent(code: string, mods: Set<ModKey>) {
    const flags = modsToFlags(mods)
    onKey({ t: 'key', code, action: 'down', ...flags })
    onKey({ t: 'key', code, action: 'up', ...flags })
  }

  function pressQuickKey(code: string) {
    dispatchKeyEvent(code, activeMods)
    if (activeMods.size > 0) setActiveMods(new Set())
  }

  // Diff the new field value against the last-seen value and forward the
  // delta. Walks a common prefix; whatever the prev value had after that
  // prefix gets removed via Backspace on the remote (Backspace pops the
  // host's input tail), and whatever the new value has after that prefix
  // gets sent as a literal text frame.
  function reconcileText(target: HTMLInputElement) {
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
      dispatchKeyEvent('Backspace', new Set())
    }
    if (added.length > 0) {
      onSend(added)
    }
    prevValueRef.current = newValue
  }

  // Mod-active branch: revert any added chars (they're commands, not
  // text) and dispatch each as a modified key event. Letters → KeyX,
  // digits → DigitN, other → bare char (agent Unicode fallback). Clears
  // mods after consumption. Removals (Backspace on a non-empty field
  // with mods active) still flow through as bare Backspace — uncommon
  // case, low risk to do the obvious thing.
  function reconcileWithMods(target: HTMLInputElement) {
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
    // Snapshot mods so a multi-char paste with Ctrl active sends e.g.
    // Ctrl+H, Ctrl+I, Ctrl rather than Ctrl+H then unmodified I.
    const mods = activeMods
    // Revert the local field so the user doesn't see the typed chars
    // (they're being routed as commands).
    target.value = prevValue
    for (let i = 0; i < removed; i++) {
      dispatchKeyEvent('Backspace', mods)
    }
    for (const ch of added) {
      dispatchKeyEvent(charToCode(ch), mods)
    }
    if (mods.size > 0) setActiveMods(new Set())
    prevValueRef.current = prevValue
  }

  function onInput(e: React.SyntheticEvent<HTMLInputElement>) {
    if (composingRef.current) return
    if (activeMods.size > 0) {
      reconcileWithMods(e.currentTarget)
    } else {
      reconcileText(e.currentTarget)
    }
  }

  function onCompositionStart() {
    composingRef.current = true
  }
  function onCompositionEnd(e: React.CompositionEvent<HTMLInputElement>) {
    composingRef.current = false
    // Composed input (CJK / VI) is always text — mods don't combine
    // meaningfully with IME-committed runs.
    reconcileText(e.currentTarget)
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
      // Enter respects sticky modifiers (Shift+Enter, Ctrl+Enter).
      dispatchKeyEvent('Enter', activeMods)
      if (activeMods.size > 0) setActiveMods(new Set())
      clearField()
    }
  }

  function onInputFocus() {
    peekedRef.current = false
  }

  function onBackspaceClick() {
    const el = inputRef.current
    if (!el) return
    if (activeMods.size > 0) {
      // Backspace with mods (e.g. Ctrl+Backspace = delete word) doesn't
      // need a local field edit — just dispatch and clear.
      dispatchKeyEvent('Backspace', activeMods)
      setActiveMods(new Set())
      el.focus()
      return
    }
    if (el.value.length > 0) {
      el.value = el.value.slice(0, -1)
      reconcileText(el)
    } else {
      dispatchKeyEvent('Backspace', new Set())
    }
    el.focus()
  }

  function onEnterClick() {
    dispatchKeyEvent('Enter', activeMods)
    if (activeMods.size > 0) setActiveMods(new Set())
    clearField()
    inputRef.current?.focus()
  }

  function onPeekClick() {
    peekedRef.current = true
    // Blur dismisses the iOS / Android soft keyboard. The strip stays
    // mounted because parent `open` is still true.
    inputRef.current?.blur()
  }

  if (!open) return null

  return (
    <div
      className="lg:hidden flex flex-col gap-1 px-2 py-1.5 bg-surface border-t border-border shrink-0"
      role="group"
      aria-label="Send text and keys to remote"
    >
      {/* Row 1: modifiers + quick keys + sheet + peek. Horizontally
          scrollable on narrow phones so all keys remain reachable; iOS
          will momentum-scroll inside this container without consuming
          taps on the underlying canvas. */}
      <div className="flex items-center gap-1 overflow-x-auto -mx-1 px-1 scrollbar-none">
        {MOD_KEYS.map((m) => (
          <ModButton
            key={m.mod}
            label={m.label}
            active={activeMods.has(m.mod)}
            onClick={() => toggleMod(m.mod)}
            aria-label={m.aria}
            aria-pressed={activeMods.has(m.mod)}
          />
        ))}
        <Divider />
        {QUICK_KEYS.map((k) => (
          <KeyButton
            key={k.code}
            label={k.label}
            onClick={() => pressQuickKey(k.code)}
            aria-label={k.aria}
          />
        ))}
        <Divider />
        <ActionButton
          onClick={onOpenSheet}
          title="More keys (F1–F12, Home, End, PgUp, PgDn)"
          aria-label="Open more-keys sheet"
        >
          <span className="text-[10px] font-semibold leading-none">F…</span>
        </ActionButton>
        <ActionButton
          onClick={onPeekClick}
          title="Hide soft keyboard, keep this bar"
          aria-label="Hide soft keyboard"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
            <rect x="1.75" y="3.25" width="12.5" height="6.5" rx="1.25" />
            <path d="M4 5.5h.01M6.5 5.5h.01M9 5.5h.01M11.5 5.5h.01M4.5 8h7" strokeLinecap="round" />
            <path d="M5.5 12.5l2.5 2 2.5-2" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </ActionButton>
      </div>

      {/* Row 2: text composer + actions. */}
      <div className="flex items-center gap-1.5">
        <input
          ref={inputRef}
          type="text"
          // Uncontrolled — the field shows the typed text live (RVNC-style)
          // while reconcileText() streams the diff. Cleared on Enter / send
          // or reverted in reconcileWithMods(). defaultValue keeps React out
          // of the value loop.
          defaultValue=""
          onInput={onInput}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={onCompositionEnd}
          onKeyDown={onKeyDown}
          onFocus={onInputFocus}
          placeholder={activeMods.size > 0 ? 'Type key for combo…' : 'Send text…'}
          aria-label="Text to send to remote machine"
          inputMode="text"
          enterKeyHint="send"
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
          spellCheck={false}
          className={[
            'flex-1 min-w-0 h-9 px-3 rounded-md',
            'bg-surface-alt border text-sm text-text-primary placeholder:text-text-muted',
            'focus:outline-none',
            activeMods.size > 0
              ? 'border-[hsl(var(--accent-primary)/0.7)]'
              : 'border-border focus:border-[hsl(var(--accent-primary)/0.6)]',
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
          title="Close keyboard bar"
          aria-label="Close keyboard bar"
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M4 6l4 4 4-4" />
          </svg>
        </ActionButton>
      </div>
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
      onTouchStart={(e) => e.preventDefault()}
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

interface KeyButtonProps {
  label: string
  onClick: () => void
  'aria-label': string
}

function KeyButton({ label, onClick, ...rest }: KeyButtonProps) {
  return (
    <button
      type="button"
      onMouseDown={(e) => e.preventDefault()}
      onTouchStart={(e) => e.preventDefault()}
      onClick={onClick}
      className={[
        'shrink-0 inline-flex items-center justify-center min-w-[2.25rem] h-8 px-2 rounded-md border',
        'bg-surface-alt border-border text-text-secondary text-xs font-medium',
        'hover:text-text-primary hover:bg-surface-hover transition-colors active:scale-95',
      ].join(' ')}
      {...rest}
    >
      {label}
    </button>
  )
}

interface ModButtonProps {
  label: string
  active: boolean
  onClick: () => void
  'aria-label': string
  'aria-pressed': boolean
}

function ModButton({ label, active, onClick, ...rest }: ModButtonProps) {
  const tone = active
    ? 'bg-[hsl(var(--accent-primary))] text-white border-[hsl(var(--accent-primary))] shadow-sm'
    : 'bg-surface-alt border-border text-text-secondary hover:text-text-primary hover:bg-surface-hover'
  return (
    <button
      type="button"
      onMouseDown={(e) => e.preventDefault()}
      onTouchStart={(e) => e.preventDefault()}
      onClick={onClick}
      className={[
        'shrink-0 inline-flex items-center justify-center min-w-[2.5rem] h-8 px-2 rounded-md border',
        'text-xs font-semibold transition-colors active:scale-95',
        tone,
      ].join(' ')}
      {...rest}
    >
      {label}
    </button>
  )
}

function Divider() {
  return <span className="shrink-0 w-px h-5 bg-border mx-0.5" aria-hidden="true" />
}
