import SectionEyebrow from '../components/section-eyebrow'

type Cell = boolean | 'partial'

interface Row {
  label: string
  oxi: Cell
  others: Cell[]
}

const COLUMNS = ['OxiRemote', 'TeamViewer', 'Chrome Remote', 'Termius', 'AnyDesk']

const ROWS: Row[] = [
  { label: 'Self-hosted, your machine only',          oxi: true,  others: [false, false, false, false] },
  { label: 'Zero config, ready in <60s',              oxi: true,  others: [false, 'partial', false, false] },
  { label: 'Browser-native PWA, no app store',        oxi: true,  others: [false, true, false, false] },
  { label: 'Terminal · Files · Preview · Desktop',    oxi: true,  others: [false, false, false, false] },
  { label: 'WebRTC + JPEG/H.264 video pipelines',     oxi: true,  others: ['partial', 'partial', false, 'partial'] },
  { label: 'No account or telemetry',                 oxi: true,  others: [false, false, false, false] },
  { label: 'Push notifications with deep links',      oxi: true,  others: [false, false, false, false] },
  { label: 'Open source · MIT',                       oxi: true,  others: [false, false, false, false] },
  { label: 'One-command install',                     oxi: true,  others: [false, false, true,  true] },
  { label: 'No port forwarding',                      oxi: true,  others: [true,  true,  false, false] },
]

function CellMark({ value, accent = false }: { value: Cell; accent?: boolean }) {
  if (value === true) {
    return (
      <span
        className={`inline-flex items-center justify-center w-7 h-7 rounded-full ${
          accent
            ? 'bg-accent text-white ring-accent-soft'
            : 'bg-surface-hover/60 text-text-secondary border border-border-soft'
        }`}
        aria-label="Yes"
      >
        <svg viewBox="0 0 16 16" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M3 8.5l3.5 3.5L13 5" />
        </svg>
      </span>
    )
  }
  if (value === 'partial') {
    return (
      <span
        className="inline-flex items-center justify-center w-7 h-7 rounded-full bg-surface-alt border border-border text-text-muted"
        aria-label="Partial"
      >
        <span className="block w-2.5 h-[2px] bg-current rounded" />
      </span>
    )
  }
  return (
    <span
      className="inline-flex items-center justify-center w-7 h-7 rounded-full text-border"
      aria-label="No"
    >
      <span className="block w-1.5 h-1.5 rounded-full bg-current opacity-60" />
    </span>
  )
}

export default function CompareTable() {
  const oxiTotal = ROWS.length
  return (
    <section id="compare" className="relative py-24 sm:py-32">
      <div className="mx-auto max-w-7xl px-5 sm:px-8">
        <div className="reveal flex flex-col gap-5 max-w-2xl">
          <SectionEyebrow index="03" label="Comparison" />
          <h2 className="heading-display text-[clamp(2rem,4.6vw,3.4rem)] text-text-primary">
            One binary does the work of five SaaS tabs.
          </h2>
          <p className="text-text-secondary text-[16px] max-w-xl leading-[1.55]">
            Most remote-access tools force a tradeoff: convenience or sovereignty,
            terminal or desktop, fast or open. OxiRemote refuses the choice.
          </p>
        </div>

        <div className="reveal mt-14 rounded-3xl border border-border bg-surface-alt/70 backdrop-blur-sm overflow-hidden ring-glass">
          <div className="overflow-x-auto">
            <table className="min-w-full text-left">
              <thead>
                <tr className="border-b border-border bg-surface/40">
                  <th
                    scope="col"
                    className="px-5 sm:px-7 py-4 text-[11px] uppercase tracking-[0.16em] text-text-muted font-medium w-[42%]"
                  >
                    Capability
                  </th>
                  {COLUMNS.map((c, i) => (
                    <th
                      key={c}
                      scope="col"
                      className={`px-3 py-4 text-[12.5px] font-semibold ${
                        i === 0 ? 'text-accent' : 'text-text-secondary'
                      }`}
                    >
                      <span className={i === 0 ? 'inline-flex items-center gap-2' : ''}>
                        {i === 0 && <span className="w-2 h-2 rounded-full bg-accent" />}
                        {c}
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {ROWS.map((row, idx) => (
                  <tr
                    key={row.label}
                    className={`border-b border-border-soft last:border-0 hover:bg-surface-hover/30 transition-colors ${
                      idx % 2 === 1 ? 'bg-surface/20' : ''
                    }`}
                  >
                    <th
                      scope="row"
                      className="px-5 sm:px-7 py-3.5 text-[14px] font-medium text-text-primary"
                    >
                      {row.label}
                    </th>
                    <td className="px-3 py-3.5">
                      <CellMark value={row.oxi} accent />
                    </td>
                    {row.others.map((c, i) => (
                      <td key={i} className="px-3 py-3.5">
                        <CellMark value={c} />
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3 px-5 sm:px-7 py-4 border-t border-border bg-surface/40">
            <p className="text-[13px] text-text-secondary">
              <span className="text-accent font-semibold">OxiRemote</span> ·{' '}
              <span className="text-text-primary tabular-nums">{oxiTotal}/{oxiTotal}</span> capabilities · zero subscriptions
            </p>
            <p className="font-mono text-[11px] text-text-muted">
              comparison reflects publicly documented features as of the v0.4 release
            </p>
          </div>
        </div>
      </div>
    </section>
  )
}
