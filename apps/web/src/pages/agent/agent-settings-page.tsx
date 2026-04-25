import { Link } from 'react-router-dom'
import { Heading } from '../../components/ui'
import { PushStatusRow } from '../../components/push-permission-banner'

// Placeholder for future host-side toggles. The auto-approve switch lives on
// the home dashboard now; nothing else needs configuration today.
export default function AgentSettingsPage() {
  return (
    <div className="p-6 max-w-4xl mx-auto">
      <Heading level={1}>Settings</Heading>
      <p className="text-sm text-text-muted mt-1">
        Host-side configuration. Persists to the agent's SQLite database.
      </p>

      <section className="mt-6 rounded-lg border border-border bg-surface p-4 text-sm text-text-secondary">
        <p>
          Auto-approve is now in the{' '}
          <Link to="/agent" className="text-accent hover:text-accent-hover">
            host dashboard header
          </Link>
          . More settings will land here as we expand the surface.
        </p>
      </section>

      <section className="mt-6 rounded-lg border border-border bg-surface p-4">
        <Heading level={2}>Push notifications</Heading>
        <p className="text-sm text-text-muted mt-1 mb-3">
          Web Push for build/shell pings. Subscription is per-device; toggling here only affects this browser.
        </p>
        <PushStatusRow />
      </section>
    </div>
  )
}
