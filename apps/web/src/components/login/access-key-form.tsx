import { lazy, Suspense, useState } from 'react'
import Button from '../ui/button'

const QrScanModal = lazy(() => import('./qr-scan-modal'))

interface Props {
  value: string
  onChange: (s: string) => void
  loading: boolean
  onSubmit: (raw: string) => void
}

const hasBarcodeDetector = typeof window !== 'undefined' && 'BarcodeDetector' in window

// Permanent keys are issued with the `sk-` prefix; OTK / pairing codes never
// start with that. The submit handler uses this to route to the correct
// backend path without forcing the user to pick a mode.
function isPermanentKey(raw: string): boolean {
  return raw.trim().toLowerCase().startsWith('sk-')
}

function sanitizeAccessKey(s: string): string {
  return s.replace(/[\s-]/g, '')
}

export default function AccessKeyForm({ value, onChange, loading, onSubmit }: Props) {
  const [scanOpen, setScanOpen] = useState(false)

  const trimmed = value.trim()
  const permanent = isPermanentKey(trimmed)
  const valid = permanent ? trimmed.length > 3 : sanitizeAccessKey(trimmed).length >= 6

  function handleSubmit() {
    if (loading || !valid) return
    onSubmit(value)
  }

  function handleQrDetect(key: string) {
    setScanOpen(false)
    onChange(key)
    if (key.trim().length >= 4) onSubmit(key)
  }

  return (
    <div>
      <div className="relative">
        <input
          id="oxi-access-key"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') handleSubmit() }}
          placeholder="sk-... or One-Time Key (ABCDEFGH)"
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          className="w-full px-3 py-3 text-base bg-surface-alt border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-accent/50 focus:ring-2 focus:ring-accent/20 pr-10"
        />
        {valid && !loading && (
          <span
            aria-hidden="true"
            className="absolute right-3 top-1/2 -translate-y-1/2 text-success"
          >
            <svg viewBox="0 0 24 24" className="w-5 h-5" fill="none" stroke="currentColor" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          </span>
        )}
      </div>

      {hasBarcodeDetector && (
        <button
          type="button"
          onClick={() => setScanOpen(true)}
          className="mt-2 flex items-center gap-1.5 text-xs text-text-muted hover:text-text-secondary transition-colors"
        >
          <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="3" width="5" height="5" rx="0.5" />
            <rect x="16" y="3" width="5" height="5" rx="0.5" />
            <rect x="3" y="16" width="5" height="5" rx="0.5" />
            <path d="M16 16h5v5" />
            <path d="M16 21v-3h3" />
            <path d="M3 10h2M7 10h2M10 3v2M10 7v2M10 10h2v2h-2zM14 10h2M14 14v2M10 14h2v4" />
          </svg>
          Scan QR
        </button>
      )}

      <Button
        variant="accent-glow"
        size="lg"
        onClick={handleSubmit}
        disabled={!valid}
        loading={loading}
        className="w-full mt-3"
      >
        {loading ? 'Connecting…' : 'Connect'}
      </Button>

      {scanOpen && (
        <Suspense fallback={null}>
          <QrScanModal
            onDetect={handleQrDetect}
            onClose={() => setScanOpen(false)}
          />
        </Suspense>
      )}
    </div>
  )
}

export { isPermanentKey, sanitizeAccessKey }
