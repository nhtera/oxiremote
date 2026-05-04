// Pure SVG sparkline — no chart library. Hand-rolled to stay within bundle
// budget (250 KB gz initial). Renders a polyline path scaled to fit within
// the given width/height. Empty or single-point data renders a flat baseline.

interface Props {
  values: number[]
  width?: number
  height?: number
  className?: string
}

export default function Sparkline({ values, width = 80, height = 24, className }: Props) {
  if (values.length === 0) {
    return <svg width={width} height={height} className={className} aria-hidden="true" />
  }

  const min = Math.min(...values)
  const max = Math.max(...values)
  const range = max - min || 1
  const pad = 2

  const pts = values.map((v, i) => {
    const x = values.length === 1
      ? width / 2
      : pad + (i / (values.length - 1)) * (width - pad * 2)
    const y = pad + (1 - (v - min) / range) * (height - pad * 2)
    return `${x.toFixed(1)},${y.toFixed(1)}`
  })

  const d = pts.join(' ')

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      className={className}
      aria-hidden="true"
    >
      <polyline
        points={d}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
        opacity="0.8"
      />
    </svg>
  )
}
