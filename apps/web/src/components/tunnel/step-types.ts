// Pure types + step ordering — kept separate from components so react-refresh
// can hot-reload the row primitives without invalidating siblings.

export type StepStatus = 'done' | 'active' | 'pending' | 'failed'

export const TUNNEL_STEPS = ['Preparing', 'Connecting', 'Tunneling', 'Verifying', 'Ready'] as const

export type StepName = (typeof TUNNEL_STEPS)[number]

export interface StepState {
  name: StepName
  status: StepStatus
  info?: string
}

export function defaultSub(name: StepName, status: StepStatus): string | undefined {
  if (status === 'done') {
    switch (name) {
      case 'Preparing': return 'Tunnel binary ready'
      case 'Connecting': return 'Session established'
      case 'Tunneling': return 'Secure tunnel up'
      case 'Verifying': return 'Reachable from edge'
      case 'Ready': return 'Listening for clients'
    }
  }
  if (status === 'pending') {
    switch (name) {
      case 'Preparing': return 'Checking tunnel binary'
      case 'Connecting': return 'Creating session'
      case 'Tunneling': return 'Starting secure tunnel'
      case 'Verifying': return 'Probing edge reachability'
      case 'Ready': return 'Waiting for first client'
    }
  }
  return undefined
}
