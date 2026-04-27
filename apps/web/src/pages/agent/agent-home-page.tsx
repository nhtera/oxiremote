import { useEffect, useState } from 'react'
import PairingCard from '../../components/pairing-card'
import ApprovalModal from '../../components/approval-modal'
import { Button, StateView } from '../../components/ui'
import ConnTab from '../../components/agent/conn-tab'
import ConnLogsTabs from '../../components/agent/conn-logs-tabs'
import InlineLogsPanel from '../../components/agent/inline-logs-panel'
import OnboardingView from '../../components/agent/onboarding-view'
import OtkConfirmModal from '../../components/agent/otk-confirm-modal'

// Host-dashboard home. Live-updates via the `/api/agent/events` SSE stream;
// initial snapshot from `/api/agent/state`. Both endpoints are localhost-only.
//
// Layout: 2-col grid at xl. Left = sticky PairingCard hero. Right = Connection
// / Logs tab switcher.

interface OtkState {
  token: string
  expires_at: number
}

type AgentState = {
  tunnel_url: string | null
  host_id: string
  label: string
  platform: string
  connected_devices: number
  auto_approve?: boolean
  desktop_enabled?: boolean
  otk?: OtkState | null
}

interface PendingDevice {
  device_id: string
  ip: string
  ua_parsed: string
  first_seen: number
}

interface PermanentKeyMeta {
  last4: string
  created_at: number
}

type AgentEvent =
  | { type: 'tunnel_url_changed'; url: string }
  | { type: 'tunnel_down'; reason: string }
  | { type: 'device_connected'; device_id: string }
  | { type: 'device_disconnected'; device_id: string }
  | { type: 'device_pending'; device_id: string; ip: string; ua_parsed: string; first_seen: number }
  | { type: 'device_approved'; device_id: string }
  | { type: 'device_rejected'; device_id: string }
  | { type: 'otk_issued'; token_prefix: string }
  | { type: 'otk_used'; token_prefix: string }
  | { type: 'otk_expired'; token_prefix: string }
  | { type: 'log_entry'; level: 'info' | 'warn' | 'error'; module: string; ts: number; msg: string }
  | { type: 'step_change'; name: string; status: string; sub?: string }
  | { type: 'tunnel_step_changed'; step: string; attempt: number; info?: string; reason?: string }
  | { type: 'health_probe'; attempt: number; status: string; elapsed_ms: number; ok: boolean }
  | { type: 'permanent_key_rotated'; last4: string }

type FetchStatus = 'loading' | 'ready' | 'error'

export default function AgentHomePage() {
  const [state, setState] = useState<AgentState | null>(null)
  const [otk, setOtk] = useState<OtkState | null>(null)
  const [pendingDevice, setPendingDevice] = useState<PendingDevice | null>(null)
  const [otkError, setOtkError] = useState<string | null>(null)
  const [tunnelHealthy, setTunnelHealthy] = useState(false)
  const [tunnelDown, setTunnelDown] = useState(false)
  const [tunnelStepReady, setTunnelStepReady] = useState(false)
  const [fetchStatus, setFetchStatus] = useState<FetchStatus>('loading')
  const [retryNonce, setRetryNonce] = useState(0)
  const [permanentKey, setPermanentKey] = useState<PermanentKeyMeta | null>(null)
  const [revealedPlaintext, setRevealedPlaintext] = useState<string | null>(null)
  const [confirmingOtk, setConfirmingOtk] = useState(false)
  const [tab, setTab] = useState<'connection' | 'logs'>('connection')

  useEffect(() => {
    let cancelled = false

    Promise.all([
      fetch('/api/agent/state').then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json() as Promise<AgentState & { tunnel_step?: { type?: string; step?: string } | null }>
      }),
      fetch('/api/agent/keys/permanent').then((r) => {
        if (r.status === 404) return null
        if (!r.ok) return null
        return r.json() as Promise<PermanentKeyMeta>
      }),
    ])
      .then(([agentData, keyMeta]) => {
        if (cancelled) return
        setState(agentData)
        setOtk(agentData.otk ?? null)
        if (
          agentData.tunnel_step?.type === 'tunnel_step_changed'
          && agentData.tunnel_step.step === 'ready'
        ) {
          setTunnelStepReady(true)
          setTunnelHealthy(true)
        }
        setPermanentKey(keyMeta)
        setFetchStatus('ready')
      })
      .catch(() => {
        if (!cancelled) setFetchStatus('error')
      })
    return () => {
      cancelled = true
    }
  }, [retryNonce])

  function retry() {
    setFetchStatus('loading')
    setRetryNonce((n) => n + 1)
  }

  useEffect(() => {
    const es = new EventSource('/api/agent/events')
    es.onmessage = (msg) => {
      try {
        const ev: AgentEvent = JSON.parse(msg.data)
        if (ev.type === 'tunnel_step_changed' && ev.step === 'ready') {
          setTunnelStepReady(true)
          setTunnelHealthy(true)
          setTunnelDown(false)
        } else if (ev.type === 'tunnel_url_changed') {
          setState((s) => (s ? { ...s, tunnel_url: ev.url } : s))
          setTunnelHealthy(false)
          setTunnelDown(false)
          setTunnelStepReady(false)
        } else if (ev.type === 'tunnel_down') {
          setTunnelDown(true)
          setTunnelHealthy(false)
        } else if (ev.type === 'health_probe') {
          if (ev.ok) setTunnelHealthy(true)
        } else if (ev.type === 'device_connected') {
          setState((s) => (s ? { ...s, connected_devices: s.connected_devices + 1 } : s))
        } else if (ev.type === 'device_disconnected') {
          setState((s) =>
            s ? { ...s, connected_devices: Math.max(0, s.connected_devices - 1) } : s,
          )
        } else if (ev.type === 'device_pending') {
          setPendingDevice({
            device_id: ev.device_id,
            ip: ev.ip,
            ua_parsed: ev.ua_parsed,
            first_seen: ev.first_seen,
          })
        } else if (ev.type === 'device_approved' || ev.type === 'device_rejected') {
          setPendingDevice((d) => (d?.device_id === ev.device_id ? null : d))
        } else if (ev.type === 'otk_expired') {
          setOtk((o) => (o ? { ...o, expires_at: 0 } : o))
        } else if (ev.type === 'permanent_key_rotated') {
          fetch('/api/agent/keys/permanent')
            .then((r) => (r.ok ? r.json() : null))
            .then((meta: PermanentKeyMeta | null) => {
              if (meta) setPermanentKey(meta)
            })
            .catch(() => { /* non-critical; stale meta is acceptable */ })
        }
      } catch {
        // drop malformed frames; server retries next event
      }
    }
    return () => es.close()
  }, [])

  const handleRegenOtk = async () => {
    setOtkError(null)
    setConfirmingOtk(false)
    try {
      const res = await fetch('/api/agent/keys/one-time', { method: 'POST' })
      if (!res.ok) throw new Error(`Failed to generate key (${res.status})`)
      const data: { token: string; expires_at: number } = await res.json()
      setOtk({ token: data.token, expires_at: data.expires_at })
    } catch (e) {
      setOtkError(e instanceof Error ? e.message : 'Failed to generate key')
    }
  }

  const handlePermanentRegen = async () => {
    setOtkError(null)
    try {
      const res = await fetch('/api/agent/keys/permanent', { method: 'POST' })
      if (!res.ok) throw new Error(`Failed to rotate permanent key (${res.status})`)
      const data: { api_key: string; last4: string; created_at: number } = await res.json()
      setRevealedPlaintext(data.api_key)
      setPermanentKey({ last4: data.last4, created_at: data.created_at })
    } catch (e) {
      setOtkError(e instanceof Error ? e.message : 'Failed to rotate permanent key')
    }
  }

  const handleApprove = async () => {
    if (!pendingDevice) return
    await fetch(`/api/agent/approvals/${pendingDevice.device_id}/approve`, { method: 'POST' })
    setPendingDevice(null)
  }

  const handleReject = async () => {
    if (!pendingDevice) return
    await fetch(`/api/agent/approvals/${pendingDevice.device_id}/reject`, { method: 'POST' })
    setPendingDevice(null)
  }

  const tunnelUrl = state?.tunnel_url ?? null
  const onboarding = !tunnelStepReady || !tunnelHealthy

  if (fetchStatus === 'error') {
    return (
      <div className="p-6 max-w-4xl mx-auto">
        <StateView
          tone="error"
          icon={
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="h-full w-full" aria-hidden="true">
              <circle cx="12" cy="12" r="10" />
              <line x1="12" y1="8" x2="12" y2="12" />
              <line x1="12" y1="16" x2="12.01" y2="16" />
            </svg>
          }
          title="Couldn't reach the agent"
          body="The local oxiremote process isn't responding. Make sure it's running, then retry."
          action={<Button variant="primary" onClick={retry}>Retry</Button>}
        />
      </div>
    )
  }

  if (onboarding) {
    return <OnboardingView tunnelDown={tunnelDown} />
  }

  return (
    <div className="p-6 max-w-6xl mx-auto">
      {tunnelDown && (
        <div className="mb-4 px-4 py-3 rounded-md bg-danger/10 border border-danger/40 text-danger text-sm font-medium">
          Tunnel went down — connections will fail. Restart the agent to reconnect.
        </div>
      )}
      <div className="grid grid-cols-1 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)] gap-6">
        <aside className="lg:sticky lg:top-6 self-start space-y-4">
          <PairingCard
            tunnelUrl={tunnelUrl}
            otkToken={otk?.token ?? null}
            otkExpiresAt={otk?.expires_at ?? null}
            onRegenerate={() => setConfirmingOtk(true)}
            errorMessage={otkError}
            permanentKey={permanentKey}
            revealedPlaintext={revealedPlaintext}
            onRegeneratePermanent={handlePermanentRegen}
            onDismissReveal={() => setRevealedPlaintext(null)}
          />
        </aside>

        <section className="space-y-4">
          <ConnLogsTabs tab={tab} onChange={setTab} />
          {tab === 'connection' && state && (
            <ConnTab
              hostId={state.host_id}
              label={state.label}
              platform={state.platform}
              connectedDevices={state.connected_devices}
              autoApprove={state.auto_approve ?? false}
              desktopEnabled={state.desktop_enabled ?? true}
              tunnelUrl={tunnelUrl}
              onAutoApproveChange={(next) =>
                setState((s) => (s ? { ...s, auto_approve: next } : s))
              }
              onDesktopEnabledChange={(next) =>
                setState((s) => (s ? { ...s, desktop_enabled: next } : s))
              }
            />
          )}
          {tab === 'logs' && <InlineLogsPanel />}
        </section>
      </div>

      {pendingDevice && (
        <ApprovalModal
          device={pendingDevice}
          onApprove={handleApprove}
          onReject={handleReject}
          onClose={() => setPendingDevice(null)}
        />
      )}

      {confirmingOtk && (
        <OtkConfirmModal
          onConfirm={handleRegenOtk}
          onCancel={() => setConfirmingOtk(false)}
        />
      )}
    </div>
  )
}

