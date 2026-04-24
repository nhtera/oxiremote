// Phase 02 wires this to `POST /api/agent/settings/auto-approve`. Phase 01
// ships the visual stub so the nav surface is complete.
export default function AgentSettingsPage() {
  return (
    <div className="p-6 max-w-4xl mx-auto">
      <h1 className="text-xl font-semibold text-text-primary">Settings</h1>
      <p className="text-sm text-text-muted mt-1">
        Host-side configuration. Persists to the agent's SQLite database.
      </p>

      <section className="mt-6 rounded-lg border border-border bg-surface p-4">
        <div className="flex items-start justify-between gap-4">
          <div>
            <div className="text-sm font-medium text-text-primary">Auto-approve new devices</div>
            <div className="text-xs text-text-muted mt-1">
              When on, devices that scan a valid one-time key join without a prompt.
            </div>
          </div>
          <button
            disabled
            className="px-3 py-1.5 text-xs rounded-md border border-border text-text-muted cursor-not-allowed"
          >
            Phase 02
          </button>
        </div>
      </section>
    </div>
  )
}
