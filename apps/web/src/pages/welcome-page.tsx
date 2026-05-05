import { Link } from 'react-router-dom'
import { listSavedHosts, formatRelative, type SavedHost } from '../lib/saved-hosts'

// First-run welcome screen. Shown when the SPA loads without a paired host
// (currentHostId is null after `fetchHost` completes). Sets the product
// frame — terminal, files, remote desktop — and points the user at the
// pairing flow with a clear primary CTA. Returning users see a Recent Hosts
// list so they can re-enter without re-pairing or re-scanning a QR.
export default function WelcomePage() {
  const recentHosts = listSavedHosts()
  return (
    <div className="relative min-h-dvh bg-dot-grid flex items-center justify-center px-6 py-10 overflow-hidden">
      <div className="relative w-full max-w-md">
        <header className="mb-8 text-center animate-hero-rise">
          <div className="relative mx-auto mb-5 w-16 h-16">
            {/* Soft halo sitting behind the icon — calm dot-grid surface stays
                visible at the edges, brand mark gets a sense of warmth. */}
            <span
              aria-hidden
              className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-44 h-44 rounded-full pointer-events-none"
              style={{
                background:
                  'radial-gradient(circle at center, color-mix(in srgb, #ff7a40 60%, transparent) 0%, color-mix(in srgb, #ff7a40 22%, transparent) 35%, transparent 70%)',
                filter: 'blur(24px)',
                opacity: 0.55,
              }}
            />
            <div className="relative inline-flex items-center justify-center w-16 h-16 rounded-[20px] bg-accent text-white shadow-[0_18px_40px_-12px_rgba(255,122,64,0.55)] ring-1 ring-white/10">
            <svg
              viewBox="0 0 24 24"
              className="w-8 h-8"
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
            <span aria-hidden className="absolute -top-1 -right-1 inline-flex w-3 h-3 rounded-full bg-success ring-2 ring-surface" />
            </div>
          </div>
          <h1
            className="font-semibold tracking-[-0.03em] text-text-primary"
            style={{ fontSize: 'var(--text-display)', lineHeight: 1.04 }}
          >
            OxiRemote
          </h1>
          <p className="mt-3 text-[15px] text-text-secondary leading-relaxed max-w-sm mx-auto">
            Your dev box, in your pocket. Terminal, files, and desktop —
            reachable from any browser, anywhere.
          </p>
          <p className="mt-2 inline-flex items-center gap-1.5 text-[11px] text-text-muted">
            <span aria-hidden className="w-1 h-1 rounded-full bg-accent" />
            Built for AI coding sessions on the go
            <span aria-hidden className="w-1 h-1 rounded-full bg-accent" />
          </p>
        </header>

        {recentHosts.length > 0 && <RecentHosts hosts={recentHosts} />}

        {recentHosts.length === 0 && (
          <div className="space-y-2 mb-8">
            <FeatureRow
              delay={1}
              icon={<path d="M4 5h16v14H4z M8 9l3 3-3 3 M14 15h2" />}
              title="Remote terminal"
              body="Multi-session shells with mobile keybar — sessions persist across reconnects."
            />
            <FeatureRow
              delay={2}
              icon={<path d="M4 4h11l5 5v11H4z M14 4v6h6 M8 13h8 M8 17h6" />}
              title="Files & Git"
              body="Edit, stage, and commit. Conflict detection keeps you from losing local work."
            />
            <FeatureRow
              delay={3}
              icon={<path d="M3 5h18v11H3z M8 19h8 M12 16v3" />}
              title="Remote desktop"
              body="Real-time screen with H.264 or JPEG fallback. Quality auto-adapts to your network."
            />
            <FeatureRow
              delay={4}
              icon={<path d="M18 8a6 6 0 0 1-6 6m0 0a6 6 0 0 1-6-6m6 6v4m-4 2h8 M12 2v2" />}
              title="Agent supervision"
              body="Get notified when Claude / Codex finishes — tap to jump straight to that session."
            />
          </div>
        )}

        <div className="space-y-3 animate-hero-rise stagger-3">
          <Link
            to="/login"
            className="group relative block w-full overflow-hidden text-center py-3.5 text-sm font-semibold text-white rounded-xl shadow-[0_14px_30px_-14px_rgba(255,122,64,0.7)] transition-all hover:translate-y-[-1px] active:translate-y-0"
            style={{
              background: 'linear-gradient(135deg, #ff8a4d 0%, #ff5e2c 100%)',
            }}
          >
            <span className="relative flex items-center justify-center gap-2">
              {recentHosts.length > 0 ? 'Pair a new device' : 'Pair this device'}
              <svg viewBox="0 0 16 16" className="w-3.5 h-3.5 transition-transform group-hover:translate-x-0.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M3 8h10" /><path d="M9 4l4 4-4 4" />
              </svg>
            </span>
          </Link>
          <Link
            to="/login?mode=key"
            className="block w-full text-center py-3 text-sm font-medium text-text-secondary border border-border rounded-xl bg-surface-alt/40 backdrop-blur-sm hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            I already have a one-time key
          </Link>
        </div>

        <p className="mt-6 inline-flex w-full items-start gap-2 px-3 py-2.5 rounded-lg bg-surface-alt/50 border border-border/60 text-[11px] text-text-muted text-left leading-relaxed">
          <svg viewBox="0 0 16 16" className="shrink-0 w-3.5 h-3.5 mt-0.5 text-text-muted" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <rect x="3" y="7" width="10" height="6" rx="1.5" />
            <path d="M5 7V5a3 3 0 0 1 6 0v2" />
          </svg>
          <span>
            End-to-end via your machine's tunnel. No data leaves it until
            you approve this device.
          </span>
        </p>
      </div>
    </div>
  )
}

function RecentHosts({ hosts }: { hosts: SavedHost[] }) {
  return (
    <section className="mb-8">
      <div className="text-[10px] font-medium uppercase tracking-wide text-text-muted mb-2 px-1">
        Recent hosts
      </div>
      <div className="space-y-2">
        {hosts.map((h) => (
          <Link
            key={h.host_id}
            to={`/h/${h.host_id}/workspace`}
            className="flex items-center gap-3 p-3 rounded-lg bg-surface-alt border border-border hover:bg-surface-hover transition-colors"
          >
            <div className="shrink-0 w-9 h-9 rounded-md bg-accent/15 text-accent flex items-center justify-center font-medium text-sm">
              {h.label.slice(0, 1).toUpperCase()}
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-text-primary truncate">
                {h.label}
              </div>
              <div className="text-xs text-text-muted">
                Last seen {formatRelative(h.last_seen)} · …{h.api_key_last4}
              </div>
            </div>
            <svg
              viewBox="0 0 24 24"
              className="shrink-0 w-4 h-4 text-text-muted"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.75"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <polyline points="9 6 15 12 9 18" />
            </svg>
          </Link>
        ))}
      </div>
    </section>
  )
}

interface FeatureRowProps {
  icon: React.ReactNode
  title: string
  body: string
  delay?: 1 | 2 | 3 | 4 | 5
}

function FeatureRow({ icon, title, body, delay }: FeatureRowProps) {
  const staggerCls = delay ? `stagger-${delay}` : ''
  return (
    <div className={`flex gap-3 p-3 rounded-xl bg-surface-alt/70 border border-border backdrop-blur-sm hover:border-accent/30 transition-colors animate-hero-rise ${staggerCls}`}>
      <div className="shrink-0 w-10 h-10 rounded-lg bg-surface flex items-center justify-center text-accent ring-1 ring-border">
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
        <div className="text-sm font-medium text-text-primary tracking-tight">{title}</div>
        <div className="text-xs text-text-secondary mt-0.5 leading-snug">{body}</div>
      </div>
    </div>
  )
}
