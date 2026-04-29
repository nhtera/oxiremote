// Tier-2 expand drawer for TerminalKeybar. Slides in above the primary strip.
// Hosts the navigation row, F-keys (F1–F12 contiguous), Ctrl-chords, symbols,
// and a Paste button that requires a synchronous user gesture on iOS.

type Key = { label: string; value: string }

const ROW_NAV: Key[] = [
  { label: 'Home', value: '\x1b[H' },
  { label: 'End', value: '\x1b[F' },
  { label: 'PgUp', value: '\x1b[5~' },
  { label: 'PgDn', value: '\x1b[6~' },
  { label: 'Ins', value: '\x1b[2~' },
  { label: 'Del', value: '\x1b[3~' },
]

const ROW_CTRL: Key[] = [
  { label: '^Z', value: '\x1a' },
  { label: '^R', value: '\x12' },
  { label: '^L', value: '\x0c' },
  { label: '^A', value: '\x01' },
  { label: '^E', value: '\x05' },
  { label: '^W', value: '\x17' },
]

// F-keys split into two rows of 6 — F6–F9 had been missing from the previous
// build; users hit them when navigating ranger / midnight commander / btop.
const ROW_FN1: Key[] = [
  { label: 'F1', value: '\x1bOP' },
  { label: 'F2', value: '\x1bOQ' },
  { label: 'F3', value: '\x1bOR' },
  { label: 'F4', value: '\x1bOS' },
  { label: 'F5', value: '\x1b[15~' },
  { label: 'F6', value: '\x1b[17~' },
]

const ROW_FN2: Key[] = [
  { label: 'F7', value: '\x1b[18~' },
  { label: 'F8', value: '\x1b[19~' },
  { label: 'F9', value: '\x1b[20~' },
  { label: 'F10', value: '\x1b[21~' },
  { label: 'F11', value: '\x1b[23~' },
  { label: 'F12', value: '\x1b[24~' },
]

const ROW_SYMBOLS: Key[] = [
  { label: '|', value: '|' },
  { label: '/', value: '/' },
  { label: '\\', value: '\\' },
  { label: '~', value: '~' },
  { label: '`', value: '`' },
  { label: '*', value: '*' },
  { label: "'", value: "'" },
  { label: '"', value: '"' },
]

type Props = {
  visible: boolean
  onSend: (bytes: string) => void
  onPaste?: () => void
}

export default function TerminalKeybarExpanded({ visible, onSend, onPaste }: Props) {
  if (!visible) return null

  return (
    <div className="flex flex-col gap-1 pt-1 border-t border-border/50">
      <Row keys={ROW_NAV} onSend={onSend} />
      <Row keys={ROW_CTRL} onSend={onSend} />
      <Row keys={ROW_FN1} onSend={onSend} />
      <Row keys={ROW_FN2} onSend={onSend} />
      <Row keys={ROW_SYMBOLS} onSend={onSend} trailing={
        onPaste ? (
          <button
            className="btn-secondary text-xs py-1 px-2 min-w-9 text-center"
            onClick={onPaste}
            title="Paste from clipboard"
            aria-label="Paste"
          >
            Paste
          </button>
        ) : null
      } />
    </div>
  )
}

function Row({
  keys,
  onSend,
  trailing,
}: {
  keys: Key[]
  onSend: (b: string) => void
  trailing?: React.ReactNode
}) {
  return (
    <div className="flex flex-wrap gap-1">
      {keys.map((k) => (
        <button
          key={k.label}
          className="btn-secondary text-xs py-1 px-2 min-w-9 text-center"
          onClick={() => onSend(k.value)}
        >
          {k.label}
        </button>
      ))}
      {trailing}
    </div>
  )
}
