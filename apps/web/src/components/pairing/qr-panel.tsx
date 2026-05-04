import { ExternalLinkIcon } from '../icons'
import { tunnelUrlAbs } from '../../lib/tunnel-url'

interface Props {
  tunnelUrl: string | null
  qrPayload: string
  otkActive: boolean
  otkExpired: boolean
}

// QR + presence-dot login URL beneath. The dot mirrors the operator's instinct
// from messaging apps ("green = available") so they can confirm the link is
// live before pasting it to a teammate.
//
// The visible text shows the host + path so the user recognises where they're
// going; the href is the full payload (including `?k=<otk>` when active) so
// clicking auto-pairs in the same way scanning the QR does. Without this
// parity, scanning works but copy-paste-link doesn't.
export default function QrPanel({ tunnelUrl, qrPayload, otkActive, otkExpired }: Props) {
  // Two URLs: href is the full payload (matches the QR), displayUrl strips
  // the OTK query so the visible text doesn't leak the token in screenshots.
  const href = otkActive && qrPayload ? qrPayload : (tunnelUrl ? `${tunnelUrlAbs(tunnelUrl)}/login` : null)
  const displayUrl = tunnelUrl ? `${tunnelUrlAbs(tunnelUrl)}/login` : null

  return (
    <div className="flex flex-col items-center gap-3 w-48 min-w-0">
      {!tunnelUrl ? (
        <div className="w-48 h-48 rounded-md border border-border bg-surface-alt flex items-center justify-center text-xs text-text-muted text-center px-3">
          Waiting for tunnel…
        </div>
      ) : (
        <div className="relative rounded-md bg-white p-2 border border-border">
          <img
            src={`/api/agent/qr?url=${encodeURIComponent(qrPayload)}`}
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
      )}

      {href && displayUrl && (
        <div className="flex flex-col items-center gap-1 max-w-full">
          <div className="text-[11px] text-text-muted">
            Scan QR or open link to sign in
          </div>
          <a
            href={href}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1.5 text-xs text-accent hover:underline font-mono max-w-full"
            title={otkActive ? `${displayUrl} (auto-signs in)` : displayUrl}
          >
            <span
              className={`inline-block w-2.5 h-2.5 rounded-full shrink-0 ${otkActive ? 'bg-success' : 'bg-text-muted'}`}
              title={otkActive ? 'Active' : 'No active key'}
            />
            <span className="truncate">{displayUrl}</span>
            <ExternalLinkIcon size={12} />
          </a>
        </div>
      )}
    </div>
  )
}
