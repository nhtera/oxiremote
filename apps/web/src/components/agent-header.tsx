import { useRef, useState } from 'react'
import TunnelStatusPill from './tunnel/status-pill'
import AgentDisconnectButton from './agent-disconnect-button'
import { useAgentVersion } from '../lib/use-agent-version'

// Quick-action menu items that POST to existing agent control endpoints.
interface QuickAction {
  label: string
  endpoint?: string
  href?: string
  danger?: boolean
}

const QUICK_ACTIONS: QuickAction[] = [
  { label: 'Reconnect tunnel', endpoint: '/api/agent/tunnel/reconnect' },
  { label: 'Stop tunnel', endpoint: '/api/agent/tunnel/disconnect' },
  { label: 'Sign out', href: '/login' },
]

// Sticky header for all /agent/* routes. Replaces the inline header previously
// embedded in AgentLayout. Shows: logo, version chip (from useAgentVersion),
// tunnel status pill, quick-actions dropdown, and the Stop-agent button.
//
// Theme toggle slot is intentionally absent (dark-only for v1).
export default function AgentHeader() {
  const version = useAgentVersion()
  const [menuOpen, setMenuOpen] = useState(false)
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  async function runAction(action: QuickAction) {
    if (!action.endpoint) return
    setMenuOpen(false)
    setBusyAction(action.label)
    try {
      await fetch(action.endpoint, { method: 'POST' })
    } catch {
      // Network error on shutdown/reconnect is expected — ignore.
    } finally {
      setBusyAction(null)
    }
  }

  // Close menu on outside click.
  function handleMenuBlur(e: React.FocusEvent<HTMLDivElement>) {
    if (!menuRef.current?.contains(e.relatedTarget as Node | null)) {
      setMenuOpen(false)
    }
  }

  return (
    <header className="sticky top-0 z-40 flex items-center gap-3 px-4 py-2.5 border-b border-border bg-surface-alt">
      {/* Logo mark */}
      <span
        aria-hidden
        className="inline-flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-white shrink-0 shadow-[0_4px_14px_-6px_rgba(255,122,64,0.55)]"
      >
        <svg
          viewBox="0 0 24 24"
          className="w-4 h-4"
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
      </span>

      {/* Brand + version chip */}
      <div className="min-w-0">
        <div className="text-sm font-semibold text-text-primary tracking-tight leading-tight flex items-center gap-2">
          <span>
            OxiRemote{' '}
            <span className="text-text-muted font-normal">host</span>
          </span>
          {version && (
            <span className="text-[10px] font-medium text-accent bg-accent/10 border border-accent/30 rounded px-1.5 py-0.5 leading-none">
              v{version.agent}
            </span>
          )}
        </div>
        <div className="text-[11px] text-text-muted leading-tight">
          localhost dashboard
        </div>
      </div>

      <div className="flex-1" />

      {/* Tunnel status */}
      <TunnelStatusPill />

      {/* Quick-actions dropdown */}
      <div
        ref={menuRef}
        className="relative"
        onBlur={handleMenuBlur}
      >
        <button
          type="button"
          aria-label="Quick actions"
          aria-expanded={menuOpen}
          aria-haspopup="menu"
          onClick={() => setMenuOpen((o) => !o)}
          className="inline-flex items-center gap-1 rounded-md px-2.5 py-1.5 text-xs font-medium text-text-secondary border border-border hover:bg-surface-hover hover:text-text-primary transition-colors"
        >
          {busyAction ? (
            <span className="text-text-muted">{busyAction}…</span>
          ) : (
            <>
              Actions
              <svg
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="w-3 h-3"
                aria-hidden="true"
              >
                <path d="M4 6l4 4 4-4" />
              </svg>
            </>
          )}
        </button>

        {menuOpen && (
          <div
            role="menu"
            className="absolute right-0 top-full mt-1 w-44 rounded-lg border border-border bg-surface-alt shadow-lg z-50 overflow-hidden"
          >
            {QUICK_ACTIONS.map((action) =>
              action.href ? (
                <a
                  key={action.label}
                  href={action.href}
                  role="menuitem"
                  onClick={() => setMenuOpen(false)}
                  className="block w-full text-left px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
                >
                  {action.label}
                </a>
              ) : (
                <button
                  key={action.label}
                  type="button"
                  role="menuitem"
                  onClick={() => void runAction(action)}
                  className={`block w-full text-left px-3 py-2 text-sm transition-colors ${
                    action.danger
                      ? 'text-danger hover:bg-danger/10'
                      : 'text-text-secondary hover:bg-surface-hover hover:text-text-primary'
                  }`}
                >
                  {action.label}
                </button>
              ),
            )}
          </div>
        )}
      </div>

      {/* Help link */}
      <a
        href="/agent/logs"
        title="View logs"
        aria-label="View logs"
        className="inline-flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
      >
        <svg
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="w-4 h-4"
          aria-hidden="true"
        >
          <circle cx="8" cy="8" r="6.5" />
          <path d="M8 7v4" />
          <circle cx="8" cy="5.5" r="0.5" fill="currentColor" stroke="none" />
        </svg>
      </a>

      {/* Stop-agent button — wires the previously-dead AgentDisconnectButton */}
      <AgentDisconnectButton />
    </header>
  )
}
