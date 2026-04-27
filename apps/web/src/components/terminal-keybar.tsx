import { useState } from 'react'
import TerminalKeybarExpanded from './terminal-keybar-expanded'

type Props = {
  onSend: (bytes: string) => void
  /** Optional callback for transient user-visible messages (clipboard deny). */
  onToast?: (msg: string) => void
}

type Key = { label: string; value: string; isCtrlTarget?: boolean }

// Primary strip — keys users reach for most. The `...` button toggles the
// tier-2 drawer (nav / ctrl-chords / F-keys).
const PRIMARY: Key[] = [
  { label: 'Esc', value: '\x1b' },
  { label: 'Tab', value: '\t' },
  { label: 'Ctrl', value: '' }, // modifier toggle — handled separately
  { label: '^C', value: '\x03' },
  { label: '↑', value: '\x1b[A' },
  { label: '↓', value: '\x1b[B' },
  { label: '↵', value: '\r' },
  { label: '|', value: '|', isCtrlTarget: true },
  { label: '/', value: '/', isCtrlTarget: true },
  { label: '~', value: '~', isCtrlTarget: true },
]

export default function TerminalKeybar({ onSend, onToast }: Props) {
  const [ctrlActive, setCtrlActive] = useState(false)
  const [expanded, setExpanded] = useState(false)

  async function handlePaste() {
    try {
      const text = await navigator.clipboard.readText()
      if (text) onSend(text)
    } catch {
      onToast?.('Clipboard access denied')
    }
  }

  function handleKey(key: Key) {
    if (key.label === 'Ctrl') {
      setCtrlActive((v) => !v)
      return
    }

    if (ctrlActive && key.isCtrlTarget && key.value.length === 1) {
      // Send Ctrl+<char>: char code & 0x1F
      const code = key.value.charCodeAt(0) & 0x1f
      onSend(String.fromCharCode(code))
      setCtrlActive(false)
      return
    }

    onSend(key.value)
  }

  function btnClass(key: Key) {
    const base = 'btn-secondary text-xs py-1 px-2 min-w-9 text-center'
    if (key.label === 'Ctrl' && ctrlActive)
      return `${base} !bg-accent/20 !text-accent !border-accent/40`
    return base
  }

  return (
    <div className="flex flex-col gap-1 shrink-0">
      <TerminalKeybarExpanded visible={expanded} onSend={onSend} />

      <div className="flex flex-wrap gap-1">
        {PRIMARY.map((key) => (
          <button key={key.label} className={btnClass(key)} onClick={() => handleKey(key)}>
            {key.label}
          </button>
        ))}
        {/* Paste button: iOS Safari requires a synchronous user-gesture to
            access navigator.clipboard.readText(). A dedicated button is the
            only reliable path on WKWebView. Hidden on desktop (md:hidden). */}
        <button
          className="btn-secondary text-xs py-1 px-2 min-w-9 text-center md:hidden"
          onClick={() => void handlePaste()}
          title="Paste from clipboard"
          aria-label="Paste"
        >
          Paste
        </button>
        <button
          className={
            expanded
              ? 'btn-secondary text-xs py-1 px-2 min-w-9 text-center !bg-accent/20 !text-accent !border-accent/40'
              : 'btn-secondary text-xs py-1 px-2 min-w-9 text-center'
          }
          onClick={() => setExpanded((v) => !v)}
          title="More keys"
          aria-expanded={expanded}
          aria-label="Toggle more keys"
        >
          …
        </button>
      </div>
    </div>
  )
}
