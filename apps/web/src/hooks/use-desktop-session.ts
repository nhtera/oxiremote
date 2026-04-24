// WebRTC + WebSocket fallback state machine for remote desktop sessions.
// Two DataChannels: "desktop" (binary, unreliable) for frame tiles,
//                  "ctrl" (text, ordered) for input events.
// Fallback: if DC "desktop" doesn't open within 5s, the server sends tiles
// as WS binary messages. Client detects via {"type":"fallback"} JSON message.

import { useCallback, useEffect, useRef, useState } from 'react'

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
  t: 'mouse' | 'wheel' | 'key' | 'quality' | 'monitor'
  [key: string]: unknown
}

interface SessionApi {
  status: DesktopStatus
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  disconnect: () => void
  attempt: number
  screenDims?: { width: number; height: number }
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
// rejects (403) any mismatch, preventing cross-session hijack.
function wsUrl(deviceId: string): string {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}/ws/desktop/${encodeURIComponent(deviceId)}`
}

export function useDesktopSession(
  hostId: string,
  deviceId: string,
  onTile: TileCallback,
): SessionApi {
  const [status, setStatus] = useState<DesktopStatus>('idle')
  const [attempt, setAttempt] = useState(0)
  const [screenDims, setScreenDims] = useState<{ width: number; height: number } | undefined>()

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

    setStatus('connecting')

    const ws = new WebSocket(wsUrl(deviceId))
    ws.binaryType = 'arraybuffer'
    wsRef.current = ws

    const pc = new RTCPeerConnection(STUN_CONFIG)
    pcRef.current = pc

    // Desktop DC: unreliable, ordered=false — latency > reliability for frames
    const desktopDc = pc.createDataChannel('desktop', {
      ordered: false,
      maxRetransmits: 0,
    })
    desktopDcRef.current = desktopDc

    // Ctrl DC: reliable ordered — input events must not be reordered/dropped
    const ctrlDc = pc.createDataChannel('ctrl', { ordered: true })
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
      if (!fallbackRef.current && !destroyedRef.current) {
        handleDisconnect()
      }
    }

    ctrlDc.onmessage = () => {
      // Server doesn't push ctrl events in v1; ignore
    }

    ws.onopen = () => {
      setStatus('signaling')
      // Create and send offer
      pc.createOffer()
        .then((offer) => pc.setLocalDescription(offer))
        .then(() => {
          if (ws.readyState === WebSocket.OPEN && pc.localDescription) {
            ws.send(JSON.stringify({ type: 'offer', sdp: pc.localDescription.sdp }))
          }
        })
        .catch(() => handleDisconnect())

      // Start 5s fallback timer
      dcOpenTimerRef.current = setTimeout(() => {
        if (desktopDcRef.current?.readyState !== 'open' && !destroyedRef.current) {
          // Server will send {"type":"fallback"} then binary tiles over WS
          fallbackRef.current = true
        }
      }, DC_OPEN_TIMEOUT_MS)
    }

    ws.onmessage = (e: MessageEvent) => {
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
          ).catch(() => handleDisconnect())
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
          break
      }
    }

    ws.onerror = () => {
      handleDisconnect()
    }

    ws.onclose = () => {
      if (!destroyedRef.current) {
        handleDisconnect()
      }
    }
  }, [deviceId, onTile, teardown]) // eslint-disable-line react-hooks/exhaustive-deps

  function handleDisconnect() {
    if (destroyedRef.current) return
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
    reconnectTimerRef.current = setTimeout(() => {
      if (!destroyedRef.current) connect()
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

  return { status, sendInput, setQuality, disconnect, attempt, screenDims }
}
