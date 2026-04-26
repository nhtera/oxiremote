import { Link } from 'react-router-dom'

// First-run welcome screen. Shown when the SPA loads without a paired host
// (currentHostId is null after `fetchHost` completes). Sets the product
// frame — terminal, files, remote desktop — and points the user at the
// pairing flow with a clear primary CTA.
export default function WelcomePage() {
  return (
    <div className="min-h-dvh flex items-center justify-center px-6 py-10">
      <div className="w-full max-w-md">
        <header className="mb-8 text-center">
          <div className="inline-flex items-center justify-center w-14 h-14 rounded-2xl bg-accent text-white mb-5 shadow-[0_10px_30px_-10px_rgba(255,122,64,0.55)]">
            <svg
              viewBox="0 0 24 24"
              className="w-7 h-7"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.25"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <rect x="3" y="4" width="18" height="13" rx="2" />
              <path d="M8 20h8" />
              <path d="M12 17v3" />
            </svg>
          </div>
          <h1 className="text-[length:var(--text-display)] font-semibold tracking-tight text-text-primary">
            OxiRemote
          </h1>
          <p className="mt-2 text-sm text-text-secondary leading-relaxed">
            Your dev box, in your pocket. Reach your terminal, files, and
            desktop from anywhere.
          </p>
        </header>

        <div className="space-y-2 mb-8">
          <FeatureRow
            icon={
              <path d="M4 5h16v14H4z M8 9l3 3-3 3 M14 15h2" />
            }
            title="Remote terminal"
            body="Multi-session shells with mobile keybar — sessions persist across reconnects."
          />
          <FeatureRow
            icon={
              <path d="M4 4h11l5 5v11H4z M14 4v6h6 M8 13h8 M8 17h6" />
            }
            title="Files & Git"
            body="Edit, stage, and commit. Conflict detection keeps you from losing local work."
          />
          <FeatureRow
            icon={
              <path d="M3 5h18v11H3z M8 19h8 M12 16v3" />
            }
            title="Remote desktop"
            body="Real-time screen with H.264 or JPEG fallback. Quality auto-adapts to your network."
          />
        </div>

        <div className="space-y-3">
          <Link
            to="/login"
            className="block w-full text-center py-3 text-sm font-medium bg-accent/15 text-accent border border-accent/30 rounded-lg hover:bg-accent/25 transition-colors"
          >
            Pair this device
          </Link>
          <Link
            to="/login?mode=key"
            className="block w-full text-center py-3 text-sm font-medium text-text-secondary border border-border rounded-lg hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            I already have a one-time key
          </Link>
        </div>

        <p className="mt-6 text-xs text-text-muted text-center leading-relaxed">
          Pairing is end-to-end via your computer's tunnel. No data leaves
          your machine until you approve this device.
        </p>
      </div>
    </div>
  )
}

interface FeatureRowProps {
  icon: React.ReactNode
  title: string
  body: string
}

function FeatureRow({ icon, title, body }: FeatureRowProps) {
  return (
    <div className="flex gap-3 p-3 rounded-lg bg-surface-alt border border-border">
      <div className="shrink-0 w-9 h-9 rounded-md bg-surface flex items-center justify-center text-accent">
        <svg
          viewBox="0 0 24 24"
          className="w-5 h-5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          {icon}
        </svg>
      </div>
      <div className="min-w-0">
        <div className="text-sm font-medium text-text-primary">{title}</div>
        <div className="text-xs text-text-secondary mt-0.5 leading-snug">{body}</div>
      </div>
    </div>
  )
}
