// Session ICE-server configuration for the desktop WebRTC hooks.
//
// The agent advertises its ICE server list (default STUN, plus an optional
// operator-configured TURN relay — `OXI_TURN_URL` et al.) in the response
// to `GET /api/hosts/{id}/desktop/capabilities`. The desktop page stores it
// here before mounting a session view; both session hooks (video + JPEG)
// read it when constructing their RTCPeerConnection. A TURN relay is what
// keeps WebRTC media working on UDP-blocked networks — the same mechanism
// Chrome Remote Desktop relies on.
//
// Module-level state (not React state) on purpose: the hooks build their
// PeerConnection inside imperative connect() callbacks that must not
// re-run when capabilities load; a stale-but-default config for the first
// session of a cold load is acceptable and self-corrects on reconnect.

export interface IceServerEntry {
  urls: string[]
  username?: string
  credential?: string
}

const DEFAULT_CONFIG: RTCConfiguration = {
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
}

let sessionIceServers: RTCIceServer[] | null = null

/** Store the agent-advertised ICE servers for subsequent connects. */
export function setSessionIceServers(servers: IceServerEntry[] | undefined | null): void {
  if (!Array.isArray(servers) || servers.length === 0) {
    sessionIceServers = null
    return
  }
  const valid = servers.filter(
    (s) => Array.isArray(s.urls) && s.urls.every((u) => typeof u === 'string' && u.length > 0),
  )
  sessionIceServers = valid.length > 0 ? valid : null
}

/** RTCConfiguration for the next PeerConnection: agent-advertised servers
 *  when available, otherwise the built-in STUN-only default. */
export function getRtcConfiguration(): RTCConfiguration {
  return sessionIceServers ? { iceServers: sessionIceServers } : DEFAULT_CONFIG
}
