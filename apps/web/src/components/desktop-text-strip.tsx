// Always-visible text composer pinned above the desktop toolbar on mobile.
// Why pinned and not behind a button: lock-screen unlock requires the user
// to type into a field that the canvas cannot host (the canvas just shows
// pixels). CRD pins the same strip; we follow.
//
// Tapping the input opens the iOS soft keyboard via a real user gesture —
// no programmatic .focus() workaround needed. Enter or the Send button
// dispatches one `{t:'text'}` ctrl frame; the field clears but stays
// focused so the operator can chain values (username then password).

import { useRef, useState } from 'react'
import Button from './ui/button'

interface Props {
  /** Dispatch a literal string as one (or chunked) `{t:'text'}` frame. */
  onSend: (text: string) => void
  /** Open the multiline / paste sheet for long input. */
  onOpenSheet: () => void
  className?: string
}

export default function DesktopTextStrip({ onSend, onOpenSheet, className }: Props) {
  const [value, setValue] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  function send() {
    if (!value) return
    onSend(value)
    setValue('')
    // Re-focus so the user can chain a follow-up entry without re-tapping.
    inputRef.current?.focus()
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter') {
      e.preventDefault()
      send()
    }
  }

  return (
    <div
      className={[
        'lg:hidden flex items-center gap-1.5 px-2 py-1.5',
        'bg-surface border-t border-border',
        className ?? '',
      ].join(' ')}
      role="group"
      aria-label="Send text to remote"
    >
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Type to send to remote…"
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
      <button
        type="button"
        // Prevent the implicit input blur on tap so iOS Safari doesn't
        // dismiss the soft keyboard when the user taps the Sheet shortcut.
        onMouseDown={(e) => e.preventDefault()}
        onClick={onOpenSheet}
        title="Multiline / paste"
        aria-label="Open multiline text sheet"
        className={[
          'shrink-0 inline-flex items-center justify-center w-9 h-9',
          'rounded-md border border-border bg-surface-alt',
          'text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors',
        ].join(' ')}
      >
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <rect x="2" y="3" width="12" height="10" rx="1" />
          <path d="M5 6h6M5 9h6M5 12h4" strokeLinecap="round" />
        </svg>
      </button>
      <Button
        variant="accent-primary"
        size="sm"
        // iOS Safari keyboard-keep-open: preventing the default mousedown
        // behavior keeps focus on the input through the tap, so the keyboard
        // stays up after Send and the user can chain another value (e.g.
        // username then password) without re-tapping.
        onMouseDown={(e) => e.preventDefault()}
        onClick={send}
        disabled={!value}
        aria-label="Send text"
        className="shrink-0 h-9 px-3"
      >
        Send
      </Button>
    </div>
  )
}
