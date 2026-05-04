import { useEffect, useRef, useState } from 'react'
import AgentDisconnectButton from './agent-disconnect-button'
import { useAgentVersion } from '../lib/use-agent-version'

// Quick-action menu items that POST to existing agent control endpoints.
interface QuickAction {
  label: string
  endpoint?: string
  href?: string
  danger?: boolean
}

// Only ship actions whose endpoints actually exist. There is no in-app
// "Reconnect tunnel" — quick-tunnel URLs rotate on restart, so reconnect
// is a process-level operation handled by re-running the binary.
const QUICK_ACTIONS: QuickAction[] = [
  { label: 'Stop sharing', endpoint: '/api/agent/tunnel/disconnect', danger: true },
  { label: 'Sign out', href: '/login' },
]

// Sticky header for all /agent/* routes — agent-level chrome only:
// brand mark, version, actions menu, stop button. Tunnel state and live
// telemetry live in TunnelTelemetryBlock on /agent home (and were
// previously duplicated here as a "Reachable" pill — three identical
// status indicators on one screen is sloppy).
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
      // Network error on shutdown/disconnect is expected — ignore.
    } finally {
      setBusyAction(null)
    }
  }

  // Close menu on outside click or Escape.
  useEffect(() => {
    if (!menuOpen) return
    const onClick = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMenuOpen(false)
    }
    document.addEventListener('mousedown', onClick)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onClick)
      document.removeEventListener('keydown', onKey)
    }
  }, [menuOpen])

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

      {/* Brand + neutral version chip (deliberately not accent-coloured so it
          doesn't compete visually with status pills elsewhere on the page) */}
      <div className="min-w-0">
        <div className="text-sm font-semibold text-text-primary tracking-tight leading-tight flex items-center gap-2">
          <span>
            OxiRemote{' '}
            <span className="text-text-muted font-normal">host</span>
          </span>
          {version && (
            <span className="font-mono text-[10px] font-medium text-text-muted bg-surface border border-border rounded px-1.5 py-0.5 leading-none">
              v{version.agent}
            </span>
          )}
        </div>
        <div className="text-[11px] text-text-muted leading-tight">
          localhost dashboard
        </div>
      </div>

      <div className="flex-1" />

      {/* Actions dropdown — single utility menu, no duplicate Logs/help nav
          (sidebar already covers those routes) */}
      <div ref={menuRef} className="relative">
        <button
          type="button"
          aria-label="Quick actions"
          aria-expanded={menuOpen}
          aria-haspopup="menu"
          onClick={() => setMenuOpen((o) => !o)}
          className="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-text-secondary border border-border hover:bg-surface-hover hover:text-text-primary transition-colors"
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
                className={`w-3 h-3 transition-transform ${menuOpen ? 'rotate-180' : ''}`}
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
            className="absolute right-0 top-full mt-1 w-52 rounded-lg border border-border bg-surface-alt shadow-[0_10px_30px_-12px_rgba(0,0,0,0.6)] z-50 overflow-hidden"
          >
            {QUICK_ACTIONS.map((action, i) => {
              const prevDanger = QUICK_ACTIONS[i - 1]?.danger
              const showDivider = action.danger && prevDanger === false
              const className = `block w-full text-left px-3 py-2 text-sm transition-colors ${
                action.danger
                  ? 'text-danger hover:bg-danger/10'
                  : 'text-text-secondary hover:bg-surface-hover hover:text-text-primary'
              }`
              const node = action.href ? (
                <a
                  key={action.label}
                  href={action.href}
                  role="menuitem"
                  onClick={() => setMenuOpen(false)}
                  className={className}
                >
                  {action.label}
                </a>
              ) : (
                <button
                  key={action.label}
                  type="button"
                  role="menuitem"
                  onClick={() => void runAction(action)}
                  className={className}
                >
                  {action.label}
                </button>
              )
              return showDivider ? (
                <div key={`${action.label}-wrap`}>
                  <div className="border-t border-border my-1" />
                  {node}
                </div>
              ) : (
                node
              )
            })}
          </div>
        )}
      </div>

      {/* Stop-agent — explicit, always-visible destructive action. Outlined
          (not filled) so it reads "danger available" rather than "press me". */}
      <AgentDisconnectButton />
    </header>
  )
}
