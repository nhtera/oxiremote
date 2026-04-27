import { TunnelProgressCard } from '../tunnel-status-card'

interface Props {
  tunnelDown: boolean
}

// Centered first-run pane shown until the tunnel is healthy. The progress
// card self-subscribes to SSE; we just frame it with copy that explains why
// the operator is waiting.
export default function OnboardingView({ tunnelDown }: Props) {
  return (
    <div className="min-h-[80vh] flex items-start justify-center pt-16 md:pt-24 px-4">
      <div className="w-full max-w-lg space-y-5">
        <div className="text-center space-y-2">
          <h1 className="text-[length:var(--text-h1)] font-semibold tracking-tight text-text-primary">
            Bringing your tunnel online
          </h1>
          <p className="text-sm text-text-secondary leading-relaxed max-w-md mx-auto">
            Cloudflare is opening a secure outbound tunnel to your machine.
            No port forwarding, no inbound exposure.
          </p>
        </div>
        {tunnelDown && (
          <div className="px-4 py-3 rounded-lg bg-danger/10 border border-danger/40 text-danger text-sm font-medium">
            Tunnel went down — connections will fail. Restart the agent to reconnect.
          </div>
        )}
        <TunnelProgressCard />
      </div>
    </div>
  )
}
