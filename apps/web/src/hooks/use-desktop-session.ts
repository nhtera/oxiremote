// WebRTC + WebSocket fallback state machine for remote desktop sessions.
// Two DataChannels: "desktop" (binary, unreliable) for frame tiles,
//                  "ctrl" (text, ordered) for input events.
// Fallback: if DC "desktop" doesn't open within 5s, the server sends tiles
// as WS binary messages. Client detects via {"type":"fallback"} JSON message.

import { useCallback, useEffect, useRef, useState } from 'react'
import { isDiscoveryMode, getCurrentTunnelUrl } from '../lib/discovery-client'
import { getActiveHost, loadApiKey, loadTunnelBase, storeTunnelBase } from '../lib/api-client'
import { isAllowedTunnelHost, getNamedTunnelAllowlist } from '../lib/url-validation'
import { shouldFastRetryOnHandshakeFailure } from '../lib/ws-fast-retry'
import { TUNNEL_URL_CHANGED_EVENT } from './use-tunnel-url-sse'

/// Subprotocol marker used to carry a Bearer api_key on WS upgrade in
/// discovery (cross-origin) mode. Browsers can't set `Authorization` on
/// the WS handshake, but they can offer subprotocols. The agent picks the
/// marker (response header) and reads the api_key from the second value.
const WS_BEARER_PROTOCOL = 'oxi-bearer-v1'

export type DesktopStatus =
  | 'idle'
  | 'connecting'
  | 'signaling'
  | 'streaming'
  | 'fallback'
  | 'reconnecting'
  | 'disconnected'

export type QualityTier = 'low' | 'med' | 'high'

export interface DesktopInputEvent {
  // 'text' is Unicode-safe literal-string injection; routed server-side to
  // `enigo.text()` and skips the dom_code_to_key path entirely. Modifier
  // flags do not apply — caller intent must be "type these characters".
  t: 'mouse' | 'wheel' | 'key' | 'text' | 'quality' | 'monitor' | 'settings'
  [key: string]: unknown
}

interface SessionApi {
  status: DesktopStatus
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  disconnect: () => void
  attempt: number
  screenDims?: { width: number; height: number }
  /// Tile side length emitted in the most recent `capabilities` message.
  /// Clients use this for tile placement (`tileX * tileSize`) so the server
  /// can change the constant without an SPA redeploy. Sanity-clamped to
  /// 64 | 128 by the consumer in case of an out-of-protocol echo.
  tileSize?: number
  /// Reason from the agent's last `captureEnded` signal (e.g. "screen capture
  /// stopped"). Set when the capture pipeline exits mid-session so the UI can
  /// show a meaningful reconnect message instead of a frozen frame.
  lastEndReason?: string
}

// Callback invoked for every raw tile binary message (DC or WS fallback).
type TileCallback = (buf: ArrayBuffer) => void

const STUN_CONFIG: RTCConfiguration = {
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
}
const DC_OPEN_TIMEOUT_MS = 5000
const RECONNECT_DELAY_MS = 1500
const MAX_ATTEMPTS = 3

// Build the WS URL. The path segment is the authenticated device's ID —
// the agent cross-checks it against the session cookie's bound device and
// rejects (403) any mismatch, preventing cross-session hijack. In discovery
// (cross-origin) mode the SPA lives on Pages but the agent's WS lives on
// the per-host tunnel — rewrite to the saved tunnel base so the upgrade
// hits the right origin.
function wsUrl(deviceId: string): string {
  const path = `/ws/desktop/${encodeURIComponent(deviceId)}`
  if (isDiscoveryMode()) {
    const base = loadTunnelBase()
    if (base) {
      const wsBase = base.replace(/^http:/, 'ws:').replace(/^https:/, 'wss:')
      return `${wsBase}${path}`
    }
  }
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

function wsProtocols(): string[] | undefined {
  if (!isDiscoveryMode()) return undefined
  const key = loadApiKey()
  if (!key) return undefined
  return [WS_BEARER_PROTOCOL, key]
}

export function useDesktopSession(
  hostId: string,
  deviceId: string,
  onTile: TileCallback,
  tier: QualityTier = 'med',
  hidpi: boolean = false,
): SessionApi {
  const [status, setStatus] = useState<DesktopStatus>('idle')
  const [attempt, setAttempt] = useState(0)
  const [screenDims, setScreenDims] = useState<{ width: number; height: number } | undefined>()
  const [tileSize, setTileSize] = useState<number | undefined>()
  const [lastEndReason, setLastEndReason] = useState<string | undefined>()

  // Refs hold live handles so reconnect logic can tear down and rebuild.
  const wsRef = useRef<WebSocket | null>(null)
  const pcRef = useRef<RTCPeerConnection | null>(null)
  const desktopDcRef = useRef<RTCDataChannel | null>(null)
  const ctrlDcRef = useRef<RTCDataChannel | null>(null)
  const dcOpenTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const fallbackRef = useRef(false)
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const attemptRef = useRef(0)
  const destroyedRef = useRef(false)
  // Distinguishes Cloudflare Quick Tunnel handshake transients (close/error
  // before onopen) from mid-session disconnects. Reset to false at the start
  // of every connect(); flipped true once `ws.onopen` fires. Pairs with
  // fastRetryUsedRef to gate the one-shot immediate retry per cycle.
  const everOpenedThisCycleRef = useRef(false)
  const fastRetryUsedRef = useRef(false)
  // Latest UI-selected tier — read from `ctrlDc.onopen` to tell the agent
  // which tier to encode at before any frame leaves. Kept in a ref (not in
  // the effect's dep list) so tier changes don't tear down the session.
  const tierRef = useRef<QualityTier>(tier)
  useEffect(() => {
    tierRef.current = tier
  }, [tier])

  // Same pattern as tier — sent BEFORE the offer so the agent picks the
  // right encoder dims from session-start. Mid-session toggle goes via the
  // ctrl DC's `{t:"settings", hidpi}` path (see `setSettings` below).
  const hidpiRef = useRef<boolean>(hidpi)
  useEffect(() => {
    hidpiRef.current = hidpi
  }, [hidpi])

  const teardown = useCallback(() => {
    if (dcOpenTimerRef.current) clearTimeout(dcOpenTimerRef.current)
    if (reconnectTimerRef.current) clearTimeout(reconnectTimerRef.current)
    desktopDcRef.current?.close()
    ctrlDcRef.current?.close()
    pcRef.current?.close()
    if (wsRef.current && wsRef.current.readyState < WebSocket.CLOSING) {
      wsRef.current.close()
    }
    desktopDcRef.current = null
    ctrlDcRef.current = null
    pcRef.current = null
    wsRef.current = null
    fallbackRef.current = false
  }, [])

  const connect = useCallback(() => {
    if (destroyedRef.current) return
    teardown()
    fallbackRef.current = false
    everOpenedThisCycleRef.current = false

    setStatus('connecting')

    const protocols = wsProtocols()
    const ws = protocols
      ? new WebSocket(wsUrl(deviceId), protocols)
      : new WebSocket(wsUrl(deviceId))
    ws.binaryType = 'arraybuffer'
    wsRef.current = ws

    const pc = new RTCPeerConnection(STUN_CONFIG)
    pcRef.current = pc

    // Desktop DC: unreliable, ordered=false — latency > reliability for frames.
    // `negotiated: true` with a matching id pins the SCTP stream so the server's
    // identically-configured DC receives what we send (and vice versa). Without
    // this, each side creates a fresh in-band stream and frames flow into a
    // one-way void (server sees its DC open; client's never receives bytes).
    const desktopDc = pc.createDataChannel('desktop', {
      ordered: false,
      maxRetransmits: 0,
      negotiated: true,
      id: 1,
    })
    desktopDcRef.current = desktopDc

    // Ctrl DC: reliable ordered — input events must not be reordered/dropped
    const ctrlDc = pc.createDataChannel('ctrl', {
      ordered: true,
      negotiated: true,
      id: 2,
    })
    ctrlDcRef.current = ctrlDc

    // ICE candidates → forward over WS
    pc.onicecandidate = (e) => {
      if (e.candidate && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ice', candidate: e.candidate.toJSON() }))
      }
    }

    desktopDc.onopen = () => {
      if (dcOpenTimerRef.current) clearTimeout(dcOpenTimerRef.current)
      attemptRef.current = 0
      setAttempt(0)
      setStatus('streaming')
    }

    desktopDc.onmessage = (e: MessageEvent<ArrayBuffer>) => {
      onTile(e.data)
    }

    desktopDc.onerror = () => {
      // DC error usually precedes close; handled in onclose
    }

    desktopDc.onclose = () => {
      // StrictMode double-mount: a stale DC belonging to a torn-down session
      // must not trigger reconnect of the current one.
      if (desktopDcRef.current !== desktopDc) return
      if (!fallbackRef.current && !destroyedRef.current) {
        handleDisconnect()
      }
    }

    ctrlDc.onopen = () => {
      // Tell the agent the current UI tier **before** any input/frame —
      // otherwise the server defaults and we burn cycles encoding at the
      // wrong quality until the user next touches the dropdown.
      try {
        ctrlDc.send(JSON.stringify({ t: 'quality', tier: tierRef.current }))
      } catch {
        // ctrlDc could have closed between onopen and send; ignore.
      }
    }

    ctrlDc.onmessage = () => {
      // Server doesn't push ctrl events in v1; ignore
    }

    ws.onopen = () => {
      if (wsRef.current !== ws) return
      everOpenedThisCycleRef.current = true
      fastRetryUsedRef.current = false
      setStatus('signaling')
      // Push the persisted HiDPI preference before the offer so the agent
      // builds the JPEG capture pipeline at the right dims from frame zero.
      ws.send(JSON.stringify({ type: 'settings', hidpi: hidpiRef.current }))
      // Create and send offer
      pc.createOffer()
        .then((offer) => pc.setLocalDescription(offer))
        .then(() => {
          if (ws.readyState === WebSocket.OPEN && pc.localDescription) {
            ws.send(JSON.stringify({ type: 'offer', sdp: pc.localDescription.sdp }))
          }
        })
        .catch(() => {
          // Stale PC from an evicted StrictMode mount must not tear down
          // the live replacement.
          if (pcRef.current !== pc) return
          handleDisconnect()
        })

      // Start 5s fallback timer
      dcOpenTimerRef.current = setTimeout(() => {
        if (desktopDcRef.current?.readyState !== 'open' && !destroyedRef.current) {
          // Server will send {"type":"fallback"} then binary tiles over WS
          fallbackRef.current = true
        }
      }, DC_OPEN_TIMEOUT_MS)
    }

    ws.onmessage = (e: MessageEvent) => {
      if (wsRef.current !== ws) return
      if (e.data instanceof ArrayBuffer) {
        // Fallback binary tile frame
        if (fallbackRef.current) {
          onTile(e.data)
        }
        return
      }

      let msg: Record<string, unknown>
      try {
        msg = JSON.parse(e.data as string) as Record<string, unknown>
      } catch {
        return
      }

      switch (msg.type) {
        case 'answer':
          pc.setRemoteDescription(
            new RTCSessionDescription({ type: 'answer', sdp: msg.sdp as string }),
          ).catch(() => {
            if (pcRef.current !== pc) return
            handleDisconnect()
          })
          break

        case 'ice':
          pc.addIceCandidate(
            new RTCIceCandidate(msg.candidate as RTCIceCandidateInit),
          ).catch(() => {})
          break

        case 'fallback':
          // Server confirmed fallback — switch to WS binary, force low quality
          fallbackRef.current = true
          setStatus('fallback')
          attemptRef.current = 0
          setAttempt(0)
          sendCtrl({ t: 'quality', tier: 'low' }, ws)
          break

        case 'capabilities':
          if (msg.width && msg.height) {
            setScreenDims({ width: msg.width as number, height: msg.height as number })
          }
          if (typeof msg.tileSize === 'number') {
            // Sanity guard against an out-of-protocol value — only 64 / 128
            // are valid grid sizes; anything else means the client and server
            // disagree about wire format and we'd rather keep the previous
            // setting than mis-paint the canvas.
            const ts = msg.tileSize as number
            if (ts === 64 || ts === 128) setTileSize(ts)
          }
          break

        case 'captureEnded': {
          // Agent's capture pipeline exited unexpectedly (permission revoked,
          // monitor unplugged). Surface the reason and trigger reconnect.
          const reason = typeof msg.reason === 'string' ? msg.reason : 'capture ended'
          setLastEndReason(reason)
          if (!destroyedRef.current) handleDisconnect()
          break
        }
      }
    }

    ws.onerror = () => {
      // Ignore errors from a stale WS — StrictMode's first-mount WS can emit
      // `error` after mount 2 has opened a new live WS.
      if (wsRef.current !== ws) return
      handleDisconnect()
    }

    ws.onclose = () => {
      // Guard against late `close` from a superseded WS (StrictMode cleanup):
      // without this, the first-mount WS tears down the second-mount session
      // after it has already handshaked successfully.
      if (wsRef.current !== ws) return
      if (!destroyedRef.current) {
        handleDisconnect()
      }
    }
  }, [deviceId, onTile, teardown]) // eslint-disable-line react-hooks/exhaustive-deps

  function handleDisconnect() {
    if (destroyedRef.current) return

    // Cloudflare Quick Tunnel handshake transients close the WS before
    // `onopen` ever fires. Burn one immediate retry per cycle without
    // bumping the user-visible attempt counter or flipping into the
    // 'reconnecting' state — the modal/UI never reacts.
    if (shouldFastRetryOnHandshakeFailure(everOpenedThisCycleRef.current, fastRetryUsedRef.current)) {
      fastRetryUsedRef.current = true
      teardown()
      queueMicrotask(() => {
        if (!destroyedRef.current) connect()
      })
      return
    }

    const next = attemptRef.current + 1
    attemptRef.current = next
    setAttempt(next)

    if (next >= MAX_ATTEMPTS) {
      teardown()
      setStatus('disconnected')
      return
    }

    setStatus('reconnecting')
    teardown()

    // On the first disconnect, attempt to refresh the tunnel base via the
    // discovery worker (handles cold-load stale URL before SSE arrives).
    // Concurrent calls from sibling hooks share the coalesced promise.
    const doReconnect = next === 1
      ? async () => {
          try {
            const fresh = await getCurrentTunnelUrl()
            const cached = loadTunnelBase()
            if (fresh && fresh !== cached) {
              const allowlist = getNamedTunnelAllowlist()
              if (isAllowedTunnelHost(fresh, allowlist)) {
                const hostId = getActiveHost()
                if (hostId) storeTunnelBase(hostId, fresh)
              }
            }
          } catch { /* discovery not configured or worker unreachable */ }
          if (!destroyedRef.current) connect()
        }
      : async () => { if (!destroyedRef.current) connect() }

    reconnectTimerRef.current = setTimeout(() => {
      void doReconnect()
    }, RECONNECT_DELAY_MS)
  }

  // Send JSON over ctrl DC (streaming) or WS text (fallback)
  function sendCtrl(ev: DesktopInputEvent, ws?: WebSocket) {
    const payload = JSON.stringify(ev)
    if (!fallbackRef.current && ctrlDcRef.current?.readyState === 'open') {
      ctrlDcRef.current.send(payload)
    } else if (wsRef.current?.readyState === WebSocket.OPEN) {
      ;(ws ?? wsRef.current).send(payload)
    }
  }

  const sendInput = useCallback((ev: DesktopInputEvent) => {
    sendCtrl(ev)
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  const setQuality = useCallback((tier: QualityTier) => {
    sendCtrl({ t: 'quality', tier })
  }, []) // eslint-disable-line

  // JPEG path supports live HiDPI flips: agent's `spawn_capture_pipeline`
  // restarts the loop on the settings watch and re-emits Capabilities so
  // the canvas resizes itself. No reconnect needed.
  const setSettings = useCallback((next: { hidpi: boolean }) => {
    sendCtrl({ t: 'settings', hidpi: next.hidpi })
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  const disconnect = useCallback(() => {
    destroyedRef.current = true
    teardown()
    setStatus('disconnected')
  }, [teardown])

  useEffect(() => {
    // Skip until /api/me has resolved the authenticated device_id.
    if (!deviceId) return
    destroyedRef.current = false
    connect()
    return () => {
      destroyedRef.current = true
      teardown()
    }
  }, [hostId, deviceId]) // eslint-disable-line react-hooks/exhaustive-deps

  // When the supervisor rotates the tunnel URL, teardown the current session
  // and reconnect so the WS upgrade uses the new tunnel base within ~3s.
  useEffect(() => {
    if (!deviceId) return
    function onTunnelUrlChanged() {
      if (destroyedRef.current) return
      teardown()
      connect()
    }
    window.addEventListener(TUNNEL_URL_CHANGED_EVENT, onTunnelUrlChanged)
    return () => window.removeEventListener(TUNNEL_URL_CHANGED_EVENT, onTunnelUrlChanged)
  // connect / teardown are stable useCallback refs; deviceId guards mount.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceId])

  return { status, sendInput, setQuality, setSettings, disconnect, attempt, screenDims, tileSize, lastEndReason }
}
