import { useState } from 'react'
import { Button, useConfirm } from './ui'

// "Stop agent" button for the host-dashboard header. Sends a POST to the
// localhost-only /api/agent/shutdown endpoint, which delays ~500 ms before
// process::exit so this fetch settles first. On success the operator sees a
// brief "Stopping…" state then loses the SSE stream — that's expected.
export default function AgentDisconnectButton() {
  const confirm = useConfirm()
  const [stopping, setStopping] = useState(false)

  const handleClick = async () => {
    const ok = await confirm({
      title: 'Stop the agent?',
      message:
        'The tunnel will close and all connected devices will disconnect. Restart the agent in your terminal to bring it back.',
      confirmText: 'Stop agent',
      danger: true,
    })
    if (!ok) return
    setStopping(true)
    try {
      await fetch('/api/agent/shutdown', { method: 'POST' })
    } catch {
      // The agent exits ~500 ms after accepting; the request often races
      // the shutdown and surfaces as a network error. Treat as success.
    }
  }

  return (
    <Button variant="danger" size="sm" onClick={handleClick} loading={stopping}>
      {stopping ? 'Stopping…' : 'Stop agent'}
    </Button>
  )
}
