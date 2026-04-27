import { useEffect, useState, type ReactNode } from 'react'
import { Heading, StatusChip } from '../../components/ui'
import AutoApproveToggle from '../../components/auto-approve-toggle'
import ProxyPortsCard from '../../components/proxy-ports-card'
import { PushStatusRow } from '../../components/push-permission-banner'

interface AutostartStatus {
  enabled: boolean
  supported: boolean
  mechanism: string | null
}

// Host-side configuration. Persists where the underlying control persists —
// auto-approve goes to the agent SQLite; quality/HiDPI defaults are per-device
// localStorage; proxy ports go to the agent. Each card calls out its scope so
// the operator isn't surprised by a setting that doesn't follow them across
// devices.
//
// Endpoints used (all localhost-only via route_scope.rs):
//   GET  /api/agent/state             — initial settings + tunnel info
//   POST /api/agent/settings/auto-approve
//   GET/POST /api/agent/proxy/ports

interface AgentState {
  tunnel_url: string | null
  host_id: string
  label: string
  platform: string
  auto_approve?: boolean
}

const QUALITY_KEY = 'oxi:desktop:quality'
const HIDPI_KEY = 'oxi:desktop:hidpi'

export default function AgentSettingsPage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [quality, setQuality] = useState<string>(() =>
    localStorage.getItem(QUALITY_KEY) ?? 'med',
  )
  const [hidpi, setHidpi] = useState<boolean>(
    () => localStorage.getItem(HIDPI_KEY) === '1',
  )
  const [autostart, setAutostart] = useState<AutostartStatus | null>(null)
  const [autostartBusy, setAutostartBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    fetch('/api/agent/state')
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json()
      })
      .then((data: AgentState) => {
        if (!cancelled) setState(data)
      })
      .catch((e: unknown) =>
        setError(e instanceof Error ? e.message : 'Failed to load state'),
      )

    fetch('/api/agent/autostart')
      .then((r) => (r.ok ? r.json() : null))
      .then((data: AutostartStatus | null) => {
        if (!cancelled && data) setAutostart(data)
      })
      .catch(() => { /* non-critical; autostart section renders disabled */ })

    return () => {
      cancelled = true
    }
  }, [])

  async function toggleAutostart(next: boolean) {
    setAutostartBusy(true)
    try {
      const res = await fetch('/api/agent/autostart', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: next }),
      })
      if (res.ok) {
        setAutostart((s) => (s ? { ...s, enabled: next } : s))
      }
    } finally {
      setAutostartBusy(false)
    }
  }

  function setQualityPersist(next: string) {
    setQuality(next)
    localStorage.setItem(QUALITY_KEY, next)
  }
  function setHidpiPersist(next: boolean) {
    setHidpi(next)
    localStorage.setItem(HIDPI_KEY, next ? '1' : '0')
  }

  return (
    <div className="p-6 max-w-4xl mx-auto space-y-5">
      <header>
        <Heading level={1}>Settings</Heading>
        <p className="text-[length:var(--text-meta)] text-text-muted mt-1">
          Host-side configuration. Some settings live on the agent (synced to
          all devices); others are per-device.
        </p>
      </header>

      {error && (
        <div className="px-3 py-2 rounded-md bg-danger/10 border border-danger/30 text-danger text-[length:var(--text-meta)]">
          {error}
        </div>
      )}

      <Section
        title="Pairing"
        scope="Host-wide"
        description="Auto-approve trusts new devices the moment they pair, skipping the approval prompt. Disable it when you want to vet each pairing."
      >
        {state && (
          <AutoApproveToggle
            enabled={state.auto_approve ?? false}
            onChange={(next) =>
              setState((s) => (s ? { ...s, auto_approve: next } : s))
            }
          />
        )}
      </Section>

      <Section
        title="Desktop streaming"
        scope="This browser"
        description="Defaults applied when you open the remote desktop. Override per-session from the desktop toolbar."
      >
        <Row label="Quality">
          <select
            value={quality}
            onChange={(e) => setQualityPersist(e.target.value)}
            className="bg-surface border border-border rounded-md px-2 py-1 text-[length:var(--text-meta)] text-text-primary"
          >
            <option value="low">Smooth (low)</option>
            <option value="med">Balanced (med)</option>
            <option value="high">Crisp (high)</option>
          </select>
        </Row>
        <Row label="HiDPI">
          <SwitchButton checked={hidpi} onToggle={() => setHidpiPersist(!hidpi)} />
        </Row>
      </Section>

      <Section
        title="Notifications"
        scope="This browser"
        description="Web Push for build / shell pings. Subscription is per-device — each browser opts in separately."
      >
        <PushStatusRow />
      </Section>

      <Section
        title="Local sites"
        scope="Host-wide"
        description="Toggle a discovered port to expose it at <tunnel>/proxy/<port>/. The upstream app handles its own auth."
      >
        <ProxyPortsCard tunnelUrl={state?.tunnel_url ?? null} />
      </Section>

      <Section
        title="Workspace"
        scope="Read-only"
        description="Files page roots here. Override with OXI_WORKSPACE env var when starting the agent."
      >
        <Row label="OXI_WORKSPACE">
          <code className="font-mono text-[length:var(--text-mono)] text-text-primary truncate">
            (set on agent startup)
          </code>
        </Row>
      </Section>

      <Section
        title="Tunnel"
        scope="Host-wide"
        description="Quick tunnel rotates on agent restart. Configure a Named Tunnel for a stable URL."
      >
        <Row label="Mode">
          <StatusChip variant="info" noDot>
            Quick
          </StatusChip>
        </Row>
        <Row label="URL">
          {state?.tunnel_url ? (
            <code className="font-mono text-[length:var(--text-mono)] text-text-primary truncate max-w-[60%]">
              {state.tunnel_url}
            </code>
          ) : (
            <span className="text-text-muted">—</span>
          )}
        </Row>
        <Row label="Named tunnel">
          <a
            href="https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/get-started/"
            target="_blank"
            rel="noopener noreferrer"
            className="text-accent hover:text-accent-hover text-[length:var(--text-meta)]"
          >
            Setup guide ↗
          </a>
        </Row>
      </Section>

      <Section
        title="Autostart"
        scope="Host-wide"
        description="Start the agent automatically at login. Uses launchd on macOS, systemd user unit on Linux, or the Windows registry."
      >
        <Row label="Start at login">
          {autostart === null ? (
            <span className="text-text-muted text-[length:var(--text-meta)]">Loading…</span>
          ) : autostart.supported ? (
            <div className="flex items-center gap-2">
              <SwitchButton
                checked={autostart.enabled}
                disabled={autostartBusy}
                onToggle={() => toggleAutostart(!autostart.enabled)}
              />
              {autostart.mechanism && (
                <span className="text-text-muted text-[length:var(--text-meta)]">
                  {autostart.mechanism}
                </span>
              )}
            </div>
          ) : (
            <span className="text-text-muted text-[length:var(--text-meta)]">
              Not supported on this platform
            </span>
          )}
        </Row>
      </Section>

      <Section
        title="About"
        scope="Read-only"
        description="Run oxiremote --version in your terminal to print the binary version. Self-update via oxiremote update."
      >
        <Row label="Host ID">
          <code className="font-mono text-[length:var(--text-mono)] text-text-primary">
            {state?.host_id ?? '—'}
          </code>
        </Row>
        <Row label="Platform">
          <span className="text-text-secondary">{state?.platform ?? '—'}</span>
        </Row>
      </Section>
    </div>
  )
}

interface SectionProps {
  title: string
  scope: 'Host-wide' | 'This browser' | 'Read-only'
  description: string
  children: ReactNode
}

function Section({ title, scope, description, children }: SectionProps) {
  return (
    <section className="rounded-lg border border-border bg-surface p-4 space-y-3">
      <div className="flex items-center justify-between gap-3">
        <Heading level={2}>{title}</Heading>
        <span className="text-[length:var(--text-meta)] text-text-muted">{scope}</span>
      </div>
      <p className="text-[length:var(--text-body)] text-text-secondary leading-relaxed">
        {description}
      </p>
      <div className="pt-1">{children}</div>
    </section>
  )
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 py-1.5 text-[length:var(--text-meta)] border-b border-border/50 last:border-0">
      <span className="text-text-muted">{label}</span>
      <div className="text-right min-w-0 flex items-center gap-2">{children}</div>
    </div>
  )
}

function SwitchButton({
  checked,
  onToggle,
  disabled,
}: {
  checked: boolean
  onToggle: () => void
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={onToggle}
      className={`relative inline-flex h-5 w-9 items-center rounded-full border transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
        checked ? 'bg-accent/30 border-accent/60' : 'bg-surface-alt border-border'
      }`}
    >
      <span
        className={`inline-block h-3 w-3 transform rounded-full transition-transform ${
          checked ? 'translate-x-5 bg-accent' : 'translate-x-1 bg-text-muted'
        }`}
      />
    </button>
  )
}
