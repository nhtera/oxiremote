// Phase 02 fills this in with `GET /api/agent/approvals/pending` + device list
// from `GET /api/devices`. Phase 01 ships the empty state only.
export default function AgentDevicesPage() {
  return (
    <div className="p-6 max-w-4xl mx-auto">
      <h1 className="text-xl font-semibold text-text-primary">Devices</h1>
      <p className="text-sm text-text-muted mt-1">
        Paired devices and pending approvals appear here.
      </p>

      <div className="mt-6 rounded-lg border border-dashed border-border p-10 text-center">
        <div className="text-sm text-text-muted">No devices yet.</div>
        <div className="text-xs text-text-muted mt-2">
          Device list + approvals ship in Phase 02.
        </div>
      </div>
    </div>
  )
}
