import { useEffect, useMemo, useState } from 'react'

// Hero-rank pairing card for the host dashboard. Combines the QR, the
// one-time key, the tunnel URL, the countdown, and the regenerate button
// into a single presentation. Replaces the separate `Tunnel URL` and
// `One-Time Key` cards from the previous layout.

interface PermanentKeyMeta {
  last4: string
  created_at: number
}

interface PairingCardProps {
  tunnelUrl: string | null
  otkToken: string | null
  otkExpiresAt: number | null // Unix seconds
  onRegenerate: () => void
  errorMessage: string | null
  permanentKey: PermanentKeyMeta | null
  revealedPlaintext: string | null
  onRegeneratePermanent: () => Promise<void>
  onDismissReveal: () => void
}

export default function PairingCard({
  tunnelUrl,
  otkToken,
  otkExpiresAt,
  onRegenerate,
  errorMessage,
  permanentKey,
  revealedPlaintext,
  onRegeneratePermanent,
  onDismissReveal,
}: PairingCardProps) {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  const [copied, setCopied] = useState<'key' | 'url' | null>(null)

  // 1 s tick keeps the countdown accurate. The card is small and re-renders
  // are cheap, but we still avoid spamming when the OTK has expired.
  useEffect(() => {
    const id = setInterval(() => {
      setNow(Math.floor(Date.now() / 1000))
    }, 1000)
    return () => clearInterval(id)
  }, [])

  const remaining = otkExpiresAt ? otkExpiresAt - now : 0
  const otkActive = !!otkToken && remaining > 0
  const expiringSoon = otkActive && remaining <= 60
  const expired = !!otkToken && remaining <= 0

  const formattedKey = useMemo(
    () => formatGroupedKey(otkToken ?? ''),
    [otkToken],
  )

  // Deep link encodes `<tunnel>/login?k=<otk>` so a phone scan auto-fills
  // the one-time key. Falls back to a bare tunnel URL when no OTK exists.
  const qrPayload =
    tunnelUrl && otkActive
      ? `${tunnelUrl.replace(/\/$/, '')}/login?k=${otkToken!}`
      : tunnelUrl ?? ''

  const handleRegenerate = () => {
    onRegenerate()
  }

  const copyToClipboard = async (
    value: string,
    kind: Exclude<typeof copied, null>,
  ) => {
    try {
      await navigator.clipboard.writeText(value)
      setCopied(kind)
      setTimeout(() => {
        setCopied((c) => (c === kind ? null : c))
      }, 1500)
    } catch {
      // Fallback for older browsers / non-secure contexts: select-and-copy
      // doesn't work without a real DOM target, so silently swallow.
    }
  }

  return (
    <section className="rounded-2xl border border-border bg-surface p-5 md:p-6 shadow-[0_8px_28px_-12px_rgba(0,0,0,0.55)]">
      {/* Header + QR share a row so the QR doesn't push the text fields below
          a tall block. Text fields below get the full inner card width — no
          more squeezing OTK/URL/permanent-key into a 190px column. */}
      <div className="flex flex-col sm:flex-row gap-5 mb-5">
        <div className="flex-1 min-w-0">
          <div className="text-[11px] uppercase tracking-[0.18em] text-text-muted font-medium">
            Pairing
          </div>
          <h2 className="text-lg font-semibold text-text-primary mt-1 tracking-tight">
            Connect a phone or tablet
          </h2>
          <p className="text-xs text-text-secondary mt-1.5 leading-relaxed">
            Scan the QR with your camera, or open the link below and paste the
            one-time key.
          </p>
          <button
            onClick={handleRegenerate}
            className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-accent/15 text-accent border border-accent/40 rounded-md hover:bg-accent/25 transition-colors"
          >
            <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12a9 9 0 1 1-3-6.7L21 8" />
              <path d="M21 3v5h-5" />
            </svg>
            New key
          </button>
        </div>
        <div className="shrink-0 self-center sm:self-start">
          <QrPanel payload={qrPayload} active={!!tunnelUrl} otkExpired={expired} />
        </div>
      </div>

      <div className="space-y-4">
        <KeyBlock
          formatted={formattedKey}
          disabled={!otkActive}
          onCopy={() => otkToken && copyToClipboard(otkToken, 'key')}
          copied={copied === 'key'}
        />

        <CountdownChip
          remaining={remaining}
          expired={expired}
          expiringSoon={expiringSoon}
          hasKey={!!otkToken}
        />

        <UrlBlock
          url={tunnelUrl}
          onCopy={() => tunnelUrl && copyToClipboard(tunnelUrl, 'url')}
          copied={copied === 'url'}
        />

        <PermanentKeyBlock
          meta={permanentKey}
          plaintext={revealedPlaintext}
          onRegenerate={onRegeneratePermanent}
          onDismiss={onDismissReveal}
        />

        {errorMessage && (
          <div className="text-xs text-danger bg-danger/10 border border-danger/30 rounded-md px-3 py-2">
            {errorMessage}
          </div>
        )}
      </div>
    </section>
  )
}

function formatGroupedKey(raw: string): string {
  const cleaned = raw.replace(/[\s-]/g, '')
  const groups: string[] = []
  for (let i = 0; i < cleaned.length; i += 4) {
    groups.push(cleaned.slice(i, i + 4))
  }
  return groups.join(' ')
}

interface KeyBlockProps {
  formatted: string
  disabled: boolean
  onCopy: () => void
  copied: boolean
}

function KeyBlock({ formatted, disabled, onCopy, copied }: KeyBlockProps) {
  return (
    <div>
      <div className="text-xs font-medium text-text-muted mb-1.5">
        One-time key
      </div>
      <div
        className={
          'flex items-center justify-between gap-3 px-3 py-3 bg-surface-alt border rounded-lg ' +
          (disabled ? 'border-border/60 opacity-60' : 'border-border')
        }
      >
        <code className="font-mono text-base md:text-lg text-text-primary tracking-[0.06em] truncate select-all">
          {formatted || '—'}
        </code>
        <button
          onClick={onCopy}
          disabled={disabled || !formatted}
          aria-label="Copy one-time key"
          className="shrink-0 inline-flex items-center gap-1 px-2 py-1 text-xs text-text-secondary hover:text-text-primary disabled:opacity-30 disabled:cursor-not-allowed"
        >
          {copied ? (
            <>
              <svg
                viewBox="0 0 24 24"
                className="w-3.5 h-3.5 text-success"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <polyline points="20 6 9 17 4 12" />
              </svg>
              Copied
            </>
          ) : (
            <>
              <svg
                viewBox="0 0 24 24"
                className="w-3.5 h-3.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.75"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <rect x="9" y="9" width="13" height="13" rx="2" />
                <path d="M5 15V5a2 2 0 0 1 2-2h10" />
              </svg>
              Copy
            </>
          )}
        </button>
      </div>
    </div>
  )
}

interface CountdownChipProps {
  remaining: number // seconds
  expired: boolean
  expiringSoon: boolean
  hasKey: boolean
}

function CountdownChip({
  remaining,
  expired,
  expiringSoon,
  hasKey,
}: CountdownChipProps) {
  if (!hasKey) {
    return (
      <div className="text-xs text-text-muted">
        No active key. Tap{' '}
        <span className="text-text-secondary font-medium">New key</span> to
        generate one.
      </div>
    )
  }
  if (expired) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-danger bg-danger/10 border border-danger/30 rounded-full px-2.5 py-1">
        <span className="w-1.5 h-1.5 rounded-full bg-danger" />
        Expired
      </span>
    )
  }
  const m = Math.floor(remaining / 60)
  const s = remaining % 60
  const tone = expiringSoon
    ? 'text-warning bg-warning/10 border-warning/30'
    : 'text-text-secondary bg-surface-alt border-border'
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-xs font-medium rounded-full px-2.5 py-1 border ${tone}`}
    >
      <span
        className={
          'w-1.5 h-1.5 rounded-full ' +
          (expiringSoon ? 'bg-warning' : 'bg-success')
        }
      />
      Expires in {m}:{s.toString().padStart(2, '0')}
    </span>
  )
}

interface UrlBlockProps {
  url: string | null
  onCopy: () => void
  copied: boolean
}

function UrlBlock({ url, onCopy, copied }: UrlBlockProps) {
  if (!url) {
    return (
      <div className="text-xs text-text-muted">
        Waiting for tunnel… The pairing link will appear here once Cloudflare
        connects.
      </div>
    )
  }
  return (
    <div>
      <div className="text-xs font-medium text-text-muted mb-1.5">
        Pairing URL
      </div>
      <div className="flex items-center gap-2 min-w-0">
        <a
          href={url}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-accent hover:underline font-mono truncate min-w-0 flex-1"
          title={url}
        >
          {url}
        </a>
        <button
          onClick={onCopy}
          aria-label="Copy pairing URL"
          className="shrink-0 inline-flex items-center gap-1 px-2 py-1 text-xs text-text-secondary hover:text-text-primary"
        >
          {copied ? (
            <>
              <svg
                viewBox="0 0 24 24"
                className="w-3.5 h-3.5 text-success"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <polyline points="20 6 9 17 4 12" />
              </svg>
              Copied
            </>
          ) : (
            <>
              <svg
                viewBox="0 0 24 24"
                className="w-3.5 h-3.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.75"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <rect x="9" y="9" width="13" height="13" rx="2" />
                <path d="M5 15V5a2 2 0 0 1 2-2h10" />
              </svg>
              Copy
            </>
          )}
        </button>
      </div>
    </div>
  )
}

interface PermanentKeyBlockProps {
  meta: PermanentKeyMeta | null
  /** Plaintext key — present only immediately after a rotation; cleared by onDismiss. */
  plaintext: string | null
  onRegenerate: () => Promise<void>
  onDismiss: () => void
}

function PermanentKeyBlock({ meta, plaintext, onRegenerate, onDismiss }: PermanentKeyBlockProps) {
  const [regenerating, setRegenerating] = useState(false)
  const [copied, setCopied] = useState(false)

  const handleRegenerate = async () => {
    setRegenerating(true)
    try {
      await onRegenerate()
    } finally {
      setRegenerating(false)
    }
  }

  const handleCopy = async () => {
    if (!plaintext) return
    try {
      await navigator.clipboard.writeText(plaintext)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // Non-secure context — silently ignore.
    }
  }

  // Masked display: short form so it fits in narrow column widths.
  // Full form (`sk-···· ···· ···· ····ab12`) overflowed and wrapped per char.
  const maskedDisplay = meta ? `sk-········${meta.last4}` : null

  return (
    <div>
      <div className="text-xs font-medium text-text-muted mb-1.5">
        Permanent API key
      </div>

      {plaintext ? (
        // Reveal state: code on its own row (full width, can wrap on long
        // tokens), warning + actions stacked underneath. Avoids the previous
        // horizontal flex that forced `break-all` to wrap one char per line.
        <div className="rounded-lg border border-warning/40 bg-warning/5 p-3 space-y-3">
          <code className="block font-mono text-xs text-text-primary tracking-wider break-all select-all bg-surface px-2.5 py-2 rounded border border-border">
            {plaintext}
          </code>
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs text-warning font-medium leading-snug">
              Only shown once. Save it now.
            </span>
            <div className="shrink-0 flex items-center gap-2">
              <button
                onClick={handleCopy}
                aria-label="Copy permanent API key"
                className="inline-flex items-center gap-1 px-2 py-1 text-xs text-text-secondary hover:text-text-primary"
              >
                {copied ? (
                  <>
                    <svg viewBox="0 0 24 24" className="w-3.5 h-3.5 text-success" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                      <polyline points="20 6 9 17 4 12" />
                    </svg>
                    Copied
                  </>
                ) : (
                  <>
                    <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
                      <rect x="9" y="9" width="13" height="13" rx="2" />
                      <path d="M5 15V5a2 2 0 0 1 2-2h10" />
                    </svg>
                    Copy
                  </>
                )}
              </button>
              <button
                onClick={onDismiss}
                className="px-2.5 py-1 text-xs font-medium bg-surface-alt border border-border text-text-secondary rounded-md hover:text-text-primary hover:bg-surface-hover transition-colors"
              >
                Done
              </button>
            </div>
          </div>
        </div>
      ) : (
        // Default state: show masked key + regenerate button
        <div className="flex items-center justify-between gap-3 px-3 py-3 bg-surface-alt border border-border rounded-lg">
          <code className="font-mono text-sm text-text-secondary tracking-wider truncate select-none">
            {maskedDisplay ?? 'Not yet generated'}
          </code>
          <button
            onClick={handleRegenerate}
            disabled={regenerating}
            className="shrink-0 px-2.5 py-1 text-xs font-medium bg-danger/10 text-danger border border-danger/30 rounded-md hover:bg-danger/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {regenerating ? 'Rotating…' : 'Regenerate'}
          </button>
        </div>
      )}
    </div>
  )
}

interface QrPanelProps {
  payload: string
  active: boolean
  /** When true, blur the QR and overlay an "Expired" pill — scanning is useless. */
  otkExpired?: boolean
}

function QrPanel({ payload, active, otkExpired }: QrPanelProps) {
  if (!active) {
    return (
      <div className="w-48 h-48 rounded-md border border-border bg-surface-alt flex items-center justify-center text-xs text-text-muted text-center px-3">
        Waiting for tunnel…
      </div>
    )
  }
  return (
    <div className="relative rounded-md bg-white p-2 border border-border">
      <img
        src={`/api/agent/qr?url=${encodeURIComponent(payload)}`}
        alt="Pairing QR code"
        className="w-44 h-44"
        style={otkExpired ? { filter: 'blur(6px)' } : undefined}
      />
      {otkExpired && (
        <div className="absolute inset-0 flex items-center justify-center rounded-md">
          <span className="px-3 py-1.5 text-xs font-semibold text-white bg-accent rounded-full shadow-lg whitespace-nowrap">
            Expired — regenerate
          </span>
        </div>
      )}
    </div>
  )
}
