import { Link } from 'react-router-dom'

// Placeholder for future host-side toggles. The auto-approve switch lives on
// the home dashboard now; nothing else needs configuration today.
export default function AgentSettingsPage() {
  return (
    <div className="p-6 max-w-4xl mx-auto">
      <h1 className="text-xl font-semibold text-text-primary">Settings</h1>
      <p className="text-sm text-text-muted mt-1">
        Host-side configuration. Persists to the agent's SQLite database.
      </p>

      <section className="mt-6 rounded-lg border border-border bg-surface p-4 text-sm text-text-secondary">
        <p>
          Auto-approve is now in the{' '}
          <Link to="/agent/home" className="text-accent hover:text-accent-hover">
            host dashboard header
          </Link>
          . More settings will land here as we expand the surface.
        </p>
      </section>
    </div>
  )
}
