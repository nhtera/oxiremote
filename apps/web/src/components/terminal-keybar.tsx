import { useCallback, useState } from 'react'
import TerminalKeybarExpanded from './terminal-keybar-expanded'

type Props = {
  onSend: (bytes: string) => void
  /** Optional callback for transient user-visible messages (clipboard deny). */
  onToast?: (msg: string) => void
  /** Returns the focused terminal's current selection text (or empty). When
   *  provided, the expanded drawer renders a "Copy" button next to Paste. */
  getSelection?: () => string
}

type Modifier = 'ctrl' | 'opt' | 'meta' | 'shift'
type ModState = 'off' | 'one' | 'lock'

interface ModifierState {
  ctrl: ModState
  opt: ModState
  meta: ModState
  shift: ModState
}

const INITIAL_MODS: ModifierState = { ctrl: 'off', opt: 'off', meta: 'off', shift: 'off' }

// Encode a key value with active modifiers. PTY input is byte-level so we
// translate to the patterns shells understand:
//   Ctrl + lowercase a-z   → char & 0x1f          (e.g. Ctrl+C → \x03)
//   Shift + lowercase a-z  → uppercase
//   Opt   + anything       → ESC prefix           (bash readline reads
//                                                  `\e\e[D` as Meta-Left
//                                                  → backward-word, etc.)
//   ⌘     + anything       → unchanged            (no PTY analogue; ⌘+C/V
//                                                  are terminal-level
//                                                  clipboard ops, not
//                                                  modifier-stacked input)
//
// Multi-char sequences (arrow keys etc.) ignore Ctrl/Shift since there's no
// portable encoding for those combos across shells; Opt's ESC prefix still
// works because shells consume the leading ESC as Meta.
function applyModifiers(value: string, mods: ModifierState): string {
  if (!value) return value

  let out = value
  if (out.length === 1) {
    if (mods.shift !== 'off' && /^[a-z]$/.test(out)) {
      out = out.toUpperCase()
    }
    if (mods.ctrl !== 'off') {
      const code = out.charCodeAt(0) & 0x1f
      out = String.fromCharCode(code)
    }
  }
  if (mods.opt !== 'off') {
    out = `\x1b${out}`
  }
  return out
}

export default function TerminalKeybar({ onSend, onToast, getSelection }: Props) {
  const [mods, setMods] = useState<ModifierState>(INITIAL_MODS)
  const [expanded, setExpanded] = useState(false)
  // Tap-detection: same modifier tapped twice within 400 ms = lock.
  const [lastTap, setLastTap] = useState<{ mod: Modifier; ts: number } | null>(null)

  const clearOneShots = useCallback(() => {
    setMods((m) => ({
      ctrl: m.ctrl === 'one' ? 'off' : m.ctrl,
      opt: m.opt === 'one' ? 'off' : m.opt,
      meta: m.meta === 'one' ? 'off' : m.meta,
      shift: m.shift === 'one' ? 'off' : m.shift,
    }))
  }, [])

  const toggleModifier = useCallback((mod: Modifier) => {
    const now = Date.now()
    setMods((m) => {
      const cur = m[mod]
      const isDoubleTap = lastTap?.mod === mod && now - lastTap.ts < 400
      const next: ModState =
        cur === 'off' ? 'one'
        : cur === 'one' && isDoubleTap ? 'lock'
        : 'off'
      return { ...m, [mod]: next }
    })
    setLastTap({ mod, ts: now })
  }, [lastTap])

  const sendKey = useCallback((value: string) => {
    onSend(applyModifiers(value, mods))
    clearOneShots()
  }, [mods, onSend, clearOneShots])

  async function handlePaste() {
    try {
      const text = await navigator.clipboard.readText()
      if (text) onSend(text)
    } catch {
      onToast?.('Clipboard access denied')
    }
  }

  async function handleCopySelection() {
    const sel = getSelection?.() ?? ''
    if (!sel) {
      onToast?.('Nothing selected')
      return
    }
    try {
      await navigator.clipboard.writeText(sel)
    } catch {
      onToast?.('Clipboard access denied')
    }
  }

  function modChipClass(state: ModState): string {
    if (state === 'lock') return 'key-chip key-chip-lock'
    if (state === 'one')  return 'key-chip key-chip-once'
    return 'key-chip'
  }

  return (
    <div className="flex flex-col gap-1.5 shrink-0">
      <TerminalKeybarExpanded
        visible={expanded}
        onSend={onSend}
        onPaste={() => void handlePaste()}
        onCopySelection={getSelection ? () => void handleCopySelection() : undefined}
      />

      {/* Row 1 — navigation + interrupt. Always visible. */}
      <div className="flex gap-1.5 overflow-x-auto -mx-0.5 px-0.5 pb-0.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        <button className="key-chip" onClick={() => sendKey('\x1b')}>Esc</button>
        <button className="key-chip" onClick={() => sendKey('\x1b[D')} aria-label="Left">←</button>
        <button className="key-chip" onClick={() => sendKey('\x1b[A')} aria-label="Up">↑</button>
        <button className="key-chip" onClick={() => sendKey('\x1b[B')} aria-label="Down">↓</button>
        <button className="key-chip" onClick={() => sendKey('\x1b[C')} aria-label="Right">→</button>
        <button className="key-chip key-chip-accent" onClick={() => sendKey('\x03')} title="Ctrl+C — interrupt">^C</button>
        <button className="key-chip" onClick={() => sendKey('\t')}>Tab</button>
        <button className="key-chip" onClick={() => sendKey('\r')} aria-label="Enter">↵</button>
        <button className="key-chip" onClick={() => sendKey('\x7f')} aria-label="Backspace">⌫</button>
      </div>

      {/* Row 2 — sticky modifiers + more. */}
      <div className="flex gap-1.5">
        <button
          className={modChipClass(mods.ctrl)}
          onClick={() => toggleModifier('ctrl')}
          title="Ctrl modifier — tap once for next key, double-tap to lock"
        >
          Ctrl
        </button>
        <button
          className={modChipClass(mods.opt)}
          onClick={() => toggleModifier('opt')}
          title="Option / Alt modifier — sends ESC prefix to next key"
        >
          Opt
        </button>
        <button
          className={modChipClass(mods.meta)}
          onClick={() => toggleModifier('meta')}
          title="Command modifier — limited PTY effect"
        >
          ⌘
        </button>
        <button
          className={modChipClass(mods.shift)}
          onClick={() => toggleModifier('shift')}
          title="Shift modifier"
        >
          Shift
        </button>
        <div className="flex-1" />
        <button
          className={expanded ? 'key-chip key-chip-once' : 'key-chip'}
          onClick={() => setExpanded((v) => !v)}
          title="More keys"
          aria-expanded={expanded}
          aria-label="Toggle more keys"
        >
          {expanded ? '×' : '…'}
        </button>
      </div>
    </div>
  )
}
