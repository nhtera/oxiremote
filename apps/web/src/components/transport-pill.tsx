import { useTransportRace } from '../hooks/use-transport-race'

interface Props {
  /** Tighter padding when squeezed next to a status pill on mobile. */
  compact?: boolean
}

// Renders the LAN-first race result as a one-line badge. Driven entirely by
// the shared `use-transport-race` hook so multiple pages stay in sync.
//
// LAN: green dot · `LAN · 12 ms`
// Tunnel: blue dot · `Tunnel · 87 ms`
// Unknown (race in flight, no LAN configured): gray dot · `—`
export default function TransportPill({ compact = false }: Props) {
  const { transport, latencyMs } = useTransportRace()

  const dot =
    transport === 'lan' ? 'bg-success' : transport === 'tunnel' ? 'bg-info' : 'bg-text-muted'
  const label = transport === 'lan' ? 'LAN' : transport === 'tunnel' ? 'Tunnel' : '—'
  const latency = latencyMs != null ? ` · ${latencyMs} ms` : ''
  const padding = compact ? 'px-1.5 py-px' : 'px-2 py-0.5'

  return (
    <span
      className={`inline-flex items-center gap-1 text-[11px] ${padding} rounded-full bg-surface-alt border border-border text-text-secondary`}
      title="LAN-first transport: probes both paths and picks the fastest."
    >
      <span className={`w-1.5 h-1.5 rounded-full ${dot}`} />
      {label}
      {latency}
    </span>
  )
}
