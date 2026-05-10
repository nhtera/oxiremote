import AutoApproveToggle from '../auto-approve-toggle'
import DevicesPanel from '../devices-panel'
import PermissionsWidget from '../permissions-widget'
import ProxyPortsCard from '../proxy-ports-card'
import { type TunnelHealth } from '../tunnel/health'
import TunnelStatusCard from '../tunnel-status-card'

interface Props {
  hostId: string
  label: string
  platform: string
  connectedDevices: number
  autoApprove: boolean
  tunnelUrl: string | null
  tunnelHealth: TunnelHealth
  onAutoApproveChange: (next: boolean) => void
}

// Right-column content for the agent home's Connection tab. Pure presentation
// over snapshot props — page owns SSE state and passes deltas down.
//
// Services strip lives in `services-strip.tsx` and is rendered ABOVE the tab
// switcher in the right pane, so it stays visible regardless of active tab.
export default function ConnTab({
  hostId,
  label,
  platform,
  connectedDevices,
  autoApprove,
  tunnelUrl,
  tunnelHealth,
  onAutoApproveChange,
}: Props) {
  return (
    <div className="space-y-4">
      <TunnelStatusCard tunnelUrl={tunnelUrl} health={tunnelHealth} />

      <Card title="Host">
        <Row k="Host ID" v={hostId} />
        <Row k="Label" v={label} />
        <Row k="Platform" v={platform} />
      </Card>

      <Card
        title={
          <span className="flex items-baseline gap-2">
            <span>Clients</span>
            <span className="text-[length:var(--text-meta)] tracking-normal normal-case text-text-secondary font-normal">
              · {connectedDevices} {connectedDevices === 1 ? 'active session' : 'active sessions'}
            </span>
          </span>
        }
      >
        <div className="flex items-center justify-between gap-3 mb-3 px-3 py-2.5 rounded-lg border border-border bg-surface-alt">
          <div className="min-w-0">
            <div className="text-sm font-medium text-text-primary leading-tight">Auto-approve new devices</div>
            <div className="text-xs text-text-muted leading-snug mt-0.5">
              Skip manual approval — newly paired devices connect immediately.
            </div>
          </div>
          <AutoApproveToggle enabled={autoApprove} onChange={onAutoApproveChange} />
        </div>
        <DevicesPanel />
      </Card>

      <Card title="Remote Desktop Permissions">
        <PermissionsWidget />
      </Card>

      <Card title="Local Sites Proxy">
        <ProxyPortsCard tunnelUrl={tunnelUrl} />
      </Card>
    </div>
  )
}

function Card({
  title,
  action,
  children,
}: {
  title: React.ReactNode
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div className="rounded-xl border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-3 mb-3">
        <div className="text-xs uppercase tracking-wide text-text-muted min-w-0">{title}</div>
        {action}
      </div>
      {children}
    </div>
  )
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex justify-between py-1 text-sm">
      <span className="text-text-muted">{k}</span>
      <span className="text-text-primary truncate ml-3 max-w-[60%]" title={v}>
        {v}
      </span>
    </div>
  )
}
