import { useEffect, useMemo, useState } from 'react'
import QrPanel from './pairing/qr-panel'
import OtkRow from './pairing/otk-row'
import PermanentRow from './pairing/permanent-row'
import Button from './ui/button'

// Hero pairing card: composes the QR panel, OTK row, and permanent-key row.
// Logic-heavy bits (countdown tick, QR payload assembly, key formatting) live
// here; presentational subcomponents live under ./pairing/.

interface PermanentKeyMeta {
  last4: string
  created_at: number
}

interface Props {
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
}: Props) {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))

  useEffect(() => {
    const id = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000)
    return () => clearInterval(id)
  }, [])

  const remaining = otkExpiresAt ? otkExpiresAt - now : 0
  const otkActive = !!otkToken && remaining > 0
  const expiringSoon = otkActive && remaining <= 60
  const expired = !!otkToken && remaining <= 0

  const formattedKey = useMemo(() => formatGroupedKey(otkToken ?? ''), [otkToken])
  const qrPayload =
    tunnelUrl && otkActive
      ? `${tunnelUrl.replace(/\/$/, '')}/login?k=${otkToken!}`
      : tunnelUrl ?? ''

  return (
    <section className="rounded-2xl border border-border bg-surface p-5 md:p-6 shadow-[0_8px_28px_-12px_rgba(0,0,0,0.55)]">
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
          <Button
            variant="accent-glow"
            size="sm"
            onClick={onRegenerate}
            className="mt-3"
            leftIcon={
              <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21 12a9 9 0 1 1-3-6.7L21 8" />
                <path d="M21 3v5h-5" />
              </svg>
            }
          >
            New key
          </Button>
        </div>
        <div className="shrink-0 self-center sm:self-start">
          <QrPanel
            tunnelUrl={tunnelUrl}
            qrPayload={qrPayload}
            otkActive={otkActive}
            otkExpired={expired}
          />
        </div>
      </div>

      <div className="space-y-4">
        <OtkRow
          formattedKey={formattedKey}
          rawKey={otkToken}
          remaining={remaining}
          expired={expired}
          expiringSoon={expiringSoon}
          hasKey={!!otkToken}
          otkActive={otkActive}
        />

        <PermanentRow
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
