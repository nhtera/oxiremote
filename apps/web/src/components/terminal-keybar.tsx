import { useState } from 'react'

type Props = {
  onSend: (bytes: string) => void
}

type Key = { label: string; value: string; isCtrlTarget?: boolean }

const ROW1: Key[] = [
  { label: 'Esc', value: '\x1b' },
  { label: 'Tab', value: '\t' },
  { label: 'Ctrl', value: '' }, // modifier toggle — handled separately
  { label: '↑', value: '\x1b[A' },
  { label: '↓', value: '\x1b[B' },
  { label: '←', value: '\x1b[D' },
  { label: '→', value: '\x1b[C' },
  { label: '|', value: '|', isCtrlTarget: true },
  { label: '/', value: '/', isCtrlTarget: true },
]

const ROW2: Key[] = [
  { label: '~', value: '~', isCtrlTarget: true },
  { label: '`', value: '`', isCtrlTarget: true },
  { label: 'PgUp', value: '\x1b[5~' },
  { label: 'PgDn', value: '\x1b[6~' },
  { label: 'Home', value: '\x1b[H' },
  { label: 'End', value: '\x1b[F' },
  { label: '\\', value: '\\', isCtrlTarget: true },
]

export default function TerminalKeybar({ onSend }: Props) {
  const [ctrlActive, setCtrlActive] = useState(false)
  const [showRow2, setShowRow2] = useState(false)

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
      <div className="flex flex-wrap gap-1">
        {ROW1.map((key) => (
          <button key={key.label} className={btnClass(key)} onClick={() => handleKey(key)}>
            {key.label}
          </button>
        ))}
        <button
          className="btn-secondary text-xs py-1 px-2 min-w-9 text-center"
          onClick={() => setShowRow2((v) => !v)}
          title="More keys"
        >
          …
        </button>
      </div>

      {showRow2 && (
        <div className="flex flex-wrap gap-1">
          {ROW2.map((key) => (
            <button key={key.label} className={btnClass(key)} onClick={() => handleKey(key)}>
              {key.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
