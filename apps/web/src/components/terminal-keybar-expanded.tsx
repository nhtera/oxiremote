// Tier-2 expand drawer for TerminalKeybar. Slides in above the primary strip;
// does not replace it. Uses `position: fixed` anchored to the safe-area inset
// so the drawer stays above the iOS soft keyboard.

type Key = { label: string; value: string }

const ROW_NAV: Key[] = [
  { label: '←', value: '\x1b[D' },
  { label: '→', value: '\x1b[C' },
  { label: 'Home', value: '\x1b[H' },
  { label: 'End', value: '\x1b[F' },
  { label: 'PgUp', value: '\x1b[5~' },
  { label: 'PgDn', value: '\x1b[6~' },
]

const ROW_CTRL: Key[] = [
  { label: '^Z', value: '\x1a' },
  { label: '^R', value: '\x12' },
  { label: '^L', value: '\x0c' },
  { label: '^A', value: '\x01' },
  { label: '^E', value: '\x05' },
  { label: '^W', value: '\x17' },
]

const ROW_FN: Key[] = [
  { label: 'F1', value: '\x1bOP' },
  { label: 'F2', value: '\x1bOQ' },
  { label: 'F3', value: '\x1bOR' },
  { label: 'F4', value: '\x1bOS' },
  { label: 'F5', value: '\x1b[15~' },
  { label: 'F10', value: '\x1b[21~' },
]

type Props = {
  visible: boolean
  onSend: (bytes: string) => void
}

export default function TerminalKeybarExpanded({ visible, onSend }: Props) {
  if (!visible) return null

  return (
    <div className="flex flex-col gap-1 pt-1 border-t border-border/50">
      <Row keys={ROW_NAV} onSend={onSend} />
      <Row keys={ROW_CTRL} onSend={onSend} />
      <Row keys={ROW_FN} onSend={onSend} />
    </div>
  )
}

function Row({ keys, onSend }: { keys: Key[]; onSend: (b: string) => void }) {
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
    </div>
  )
}
