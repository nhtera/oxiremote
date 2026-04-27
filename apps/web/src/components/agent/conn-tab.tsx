import AutoApproveToggle from '../auto-approve-toggle'
import DevicesPanel from '../devices-panel'
import PermissionsWidget from '../permissions-widget'
import ProxyPortsCard from '../proxy-ports-card'
import StatusChip from '../ui/status-chip'
import { TerminalIcon, RemoteDesktopIcon } from '../icons'
import ServiceCard from './service-card'
import DesktopServiceToggle from './desktop-service-toggle'

interface Props {
  hostId: string
  label: string
  platform: string
  connectedDevices: number
  autoApprove: boolean
  desktopEnabled: boolean
  tunnelUrl: string | null
  onAutoApproveChange: (next: boolean) => void
  onDesktopEnabledChange: (next: boolean) => void
}

// Right-column content for the agent home's Connection tab. Pure presentation
// over snapshot props — page owns SSE state and passes deltas down.
export default function ConnTab({
  hostId,
  label,
  platform,
  connectedDevices,
  autoApprove,
  desktopEnabled,
  tunnelUrl,
  onAutoApproveChange,
  onDesktopEnabledChange,
}: Props) {
  return (
    <div className="space-y-4">
      <div className="grid gap-3">
        <ServiceCard
          icon={<TerminalIcon size={18} />}
          title="Remote Terminal"
          subtitle="Full shell access · always on"
          trailing={<StatusChip variant="online">On</StatusChip>}
          emphasized
        />
        <ServiceCard
          icon={<RemoteDesktopIcon size={18} />}
          title="Remote Desktop"
          subtitle="Control screen, mouse, keyboard"
          trailing={
            <DesktopServiceToggle
              enabled={desktopEnabled}
              onChange={onDesktopEnabledChange}
            />
          }
        />
      </div>

      <Card title="Host">
        <Row k="Host ID" v={hostId} />
        <Row k="Label" v={label} />
        <Row k="Platform" v={platform} />
      </Card>

      <Card
        title="Connected Devices"
        action={
          <AutoApproveToggle enabled={autoApprove} onChange={onAutoApproveChange} />
        }
      >
        <div className="flex items-baseline gap-2 mb-3">
          <div className="text-[length:var(--text-display)] font-semibold text-text-primary leading-none">
            {connectedDevices}
          </div>
          <div className="text-[length:var(--text-meta)] text-text-muted">
            active terminal/preview sessions
          </div>
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
  title: string
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div className="rounded-lg border border-border bg-surface p-4">
      <div className="flex items-center justify-between gap-3 mb-3">
        <div className="text-xs uppercase tracking-wide text-text-muted">{title}</div>
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
