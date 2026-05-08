// On-screen keyboard sheet for the remote desktop. Bottom-sheet with three
// fixed regions (modifiers, navigation, function keys) plus a free-text input
// that fires per-character key events. Sticky modifiers stack until consumed
// by the next non-modifier dispatch — same model as DesktopToolbar's 8-key row.
//
// Wire format reuses use-desktop-session's DesktopInputEvent: t='key', code,
// action='down'|'up', plus ctrl/alt/shift/meta booleans.

import { useState } from 'react'
import type { DesktopInputEvent } from '../hooks/use-desktop-session'

interface Props {
  open: boolean
  onClose: () => void
  onKeyEvent: (ev: DesktopInputEvent) => void
}

type ModKey = 'ctrl' | 'alt' | 'shift' | 'meta'

interface KeyDef {
  label: string
  code: string
  mod?: ModKey
}

const MODIFIER_KEYS: KeyDef[] = [
  { label: 'Ctrl', code: '', mod: 'ctrl' },
  { label: 'Alt', code: '', mod: 'alt' },
  { label: 'Shift', code: '', mod: 'shift' },
  { label: '⌘', code: '', mod: 'meta' },
  { label: 'Esc', code: 'Escape' },
  { label: 'Tab', code: 'Tab' },
]

const NAV_KEYS: KeyDef[] = [
  { label: '←', code: 'ArrowLeft' },
  { label: '↑', code: 'ArrowUp' },
  { label: '↓', code: 'ArrowDown' },
  { label: '→', code: 'ArrowRight' },
  { label: 'Home', code: 'Home' },
  { label: 'End', code: 'End' },
  { label: 'PgUp', code: 'PageUp' },
  { label: 'PgDn', code: 'PageDown' },
]

const FN_KEYS: KeyDef[] = Array.from({ length: 12 }, (_, i) => ({
  label: `F${i + 1}`,
  code: `F${i + 1}`,
}))

export default function DesktopOnscreenKeyboard({
  open,
  onClose,
  onKeyEvent,
}: Props) {
  const [activeMods, setActiveMods] = useState<Set<ModKey>>(new Set())
  const [text, setText] = useState('')

  if (!open) return null

  function toggleMod(mod: ModKey) {
    setActiveMods((prev) => {
      const next = new Set(prev)
      if (next.has(mod)) next.delete(mod)
      else next.add(mod)
      return next
    })
  }

  function dispatchKey(key: KeyDef) {
    if (key.mod) {
      toggleMod(key.mod)
      return
    }
    const ev: DesktopInputEvent = {
      t: 'key',
      code: key.code,
      action: 'down',
      ctrl: activeMods.has('ctrl'),
      alt: activeMods.has('alt'),
      shift: activeMods.has('shift'),
      meta: activeMods.has('meta'),
    }
    onKeyEvent(ev)
    onKeyEvent({ ...ev, action: 'up' })
    if (activeMods.size > 0) setActiveMods(new Set())
  }

  function sendText() {
    if (!text) return
    // One `{t:'text'}` ctrl frame — agent routes via `enigo.text()` for
    // Unicode-safe injection. Avoids the per-char dom_code_to_key path that
    // previously dropped punctuation like `!@#$`.
    onKeyEvent({ t: 'text', s: text })
    setText('')
  }

  return (
    <div
      className="fixed inset-0 z-40 flex items-end justify-center"
      onClick={onClose}
    >
      <div className="absolute inset-0 bg-black/50" />

      <div
        className="relative z-10 w-full max-w-2xl bg-surface border border-border rounded-t-xl p-4 pb-safe space-y-3"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <span className="text-[length:var(--text-h2)] font-medium text-text-primary">
            Keyboard
          </span>
          <button
            onClick={onClose}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded"
            aria-label="Close keyboard"
          >
            ✕
          </button>
        </div>

        <KeyRow
          keys={MODIFIER_KEYS}
          activeMods={activeMods}
          onPress={dispatchKey}
        />
        <KeyRow keys={NAV_KEYS} activeMods={activeMods} onPress={dispatchKey} />
        <div className="grid grid-cols-6 gap-1">
          {FN_KEYS.map((k) => (
            <KeyButton
              key={k.label}
              label={k.label}
              onClick={() => dispatchKey(k)}
            />
          ))}
        </div>

        <div className="flex gap-2 pt-1">
          <input
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                sendText()
              }
            }}
            placeholder="Type text and press Send"
            className="flex-1 bg-surface-alt border border-border rounded-md px-2 py-1.5 text-[length:var(--text-body)] text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent/50"
          />
          <button
            onClick={sendText}
            disabled={!text}
            className="px-3 py-1.5 text-[length:var(--text-meta)] bg-accent/15 text-accent border border-accent/30 rounded-md hover:bg-accent/25 disabled:opacity-50 transition-colors"
          >
            Send
          </button>
        </div>
      </div>
    </div>
  )
}

interface KeyRowProps {
  keys: KeyDef[]
  activeMods: Set<ModKey>
  onPress: (k: KeyDef) => void
}

function KeyRow({ keys, activeMods, onPress }: KeyRowProps) {
  return (
    <div className="flex gap-1 flex-wrap">
      {keys.map((k) => {
        const active = !!k.mod && activeMods.has(k.mod)
        return (
          <KeyButton
            key={k.label}
            label={k.label}
            active={active}
            onClick={() => onPress(k)}
            className="flex-1 min-w-[3rem]"
          />
        )
      })}
    </div>
  )
}

interface KeyButtonProps {
  label: string
  active?: boolean
  onClick: () => void
  className?: string
}

function KeyButton({ label, active, onClick, className }: KeyButtonProps) {
  const base =
    'py-2 text-[length:var(--text-meta)] font-medium rounded-md border transition-colors select-none active:scale-95'
  // Active modifier = filled coral. Previously a faint /20 tint that was
  // easy to miss on small screens; the operator needs an unambiguous "this
  // mod will fire on the next key" signal so they don't double-tap by accident.
  const tone = active
    ? 'bg-[hsl(var(--accent-primary))] text-white border-[hsl(var(--accent-primary))] shadow-sm'
    : 'bg-surface-alt text-text-secondary border-border hover:bg-surface-hover hover:text-text-primary'
  const cls = [base, tone, className ?? ''].filter(Boolean).join(' ')
  return (
    <button onClick={onClick} className={cls} aria-pressed={active ? true : undefined}>
      {label}
    </button>
  )
}
