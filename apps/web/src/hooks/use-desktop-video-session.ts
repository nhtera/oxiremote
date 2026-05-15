// WebRTC session hook for the H.264 video-track pipeline.
//
// Sibling to `use-desktop-session.ts` (JPEG-over-DataChannel). The server
// announces its chosen pipeline via the `pipeline` signaling message; the
// main page mounts whichever hook matches.
//
// Architecture (research memo option 1 — native decoder path):
// - Client's first SDP offer adds a `recvonly` video transceiver so the
//   server can attach its `TrackLocalStaticSample` without renegotiation.
// - Ctrl DC stays text-only (same as JPEG path) for input events + tier.
// - Incoming video arrives as a standard `MediaStreamTrack`; we pipe it to
//   a hidden `<video>` element and use `requestVideoFrameCallback` to blit
//   each decoded frame to the on-screen canvas.
//
// Upgrade path (future): swap the `<video>` + rVFC hop for
// `RTCRtpScriptTransform` + WebCodecs `VideoDecoder` if benchmarking shows
// the compositor hop costs > 5 ms. See phase-03-h264 plan.

import { useCallback, useEffect, useRef, useState } from 'react'

import type {
  DesktopInputEvent,
  DesktopPipelineInfo,
  DesktopStatus,
  HostLockState,
  QualityTier,
} from './use-desktop-session'
import {
  DESKTOP_FALLBACK_PENDING_EVENT,
  setForceJpeg,
} from './use-desktop-session'
import {
  probeCodecSdpSupport,
  supportsAv1Video,
  supportsH264Video,
  supportsVp9Video,
} from './codec-detect'
import { isDiscoveryMode, getCurrentTunnelUrl } from '../lib/discovery-client'
import { getActiveHost, loadApiKey, loadTunnelBase, storeTunnelBase } from '../lib/api-client'
import { isAllowedTunnelHost, getNamedTunnelAllowlist } from '../lib/url-validation'
import { shouldFastRetryOnHandshakeFailure } from '../lib/ws-fast-retry'
import { TUNNEL_URL_CHANGED_EVENT } from './use-tunnel-url-sse'

// Re-export for backwards compatibility — callers that imported supportsH264Video
// directly from this module continue to work after the move to codec-detect.ts.
export { supportsAv1Video, supportsH264Video, supportsVp9Video }

const WS_BEARER_PROTOCOL = 'oxi-bearer-v1'

interface VideoSessionApi {
  status: DesktopStatus
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  /** Phase-02a: client-driven audio mute. `false` tears down the audio
   *  pipeline server-side via `UserToggleOff`; `true` is a no-op (re-enable
   *  needs reconnect — matches hidpi-flip / H.264 fallback policy). */
  toggleAudio: (enabled: boolean) => void
  disconnect: () => void
  attempt: number
  /** Set once the agent announces the stream dimensions via `capabilities`. */
  screenDims?: { width: number; height: number }
  /** Latest pipeline info — see `use-desktop-session.ts`. */
  pipelineInfo?: DesktopPipelineInfo
  /** macOS host-lock state — see `use-desktop-session.ts`. */
  hostLockState: HostLockState
  /** True when the agent reports OS Accessibility permission is missing. */
  accessibilityMissing: boolean
}

/** Callback invoked once per decoded video frame. Caller draws to canvas. */
type FrameCallback = (bitmap: VideoFrame | HTMLVideoElement, video: HTMLVideoElement) => void

const STUN_CONFIG: RTCConfiguration = {
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
}
const RECONNECT_DELAY_MS = 1500
const MAX_ATTEMPTS = 3

function wsUrl(
  deviceId: string,
  forcePipeline?: 'jpeg' | 'h264' | 'vp9' | 'av1' | 'auto',
): string {
  const path = `/ws/desktop/${encodeURIComponent(deviceId)}`
  // Server validates `?force_pipeline=jpeg|h264|vp9|av1|auto`; unknown values
  // fall back to the operator preference, so it's safe to always append.
  const query = forcePipeline ? `?force_pipeline=${forcePipeline}` : ''
  if (isDiscoveryMode()) {
    const base = loadTunnelBase()
    if (base) {
      const wsBase = base.replace(/^http:/, 'ws:').replace(/^https:/, 'wss:')
      return `${wsBase}${path}${query}`
    }
  }
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}${query}`
}

function wsProtocols(): string[] | undefined {
  if (!isDiscoveryMode()) return undefined
  const key = loadApiKey()
  if (!key) return undefined
  return [WS_BEARER_PROTOCOL, key]
}

export function useDesktopVideoSession(
  hostId: string,
  deviceId: string,
  onFrame: FrameCallback,
  tier: QualityTier = 'med',
  hidpi: boolean = false,
  // `audio` = user's session-start intent (start unmuted vs muted). Sent in
  // `capabilitiesClient.audio` so the agent can pick the initial mute state
  // for its writer task. Mid-session toggles never re-run this — the WS
  // `audioToggle` message flips a server-side atomic with no renegotiation.
  audio: boolean = false,
  forcePipeline?: 'h264' | 'vp9' | 'av1' | 'jpeg' | 'auto',
  // `audioInfra` = operator allows audio on this session (probe + DB +
  // pipeline). When true we ALWAYS add the recvonly audio transceiver to the
  // offer so the agent has a sendonly counterpart bound and can unmute mid-
  // session. Decoupled from `audio` so a user who opens muted can still
  // unmute instantly. When false, the transceiver is elided entirely
  // (matches the no-audio agent gate).
  audioInfra: boolean = false,
): VideoSessionApi {
  const [status, setStatus] = useState<DesktopStatus>('idle')
  const [attempt, setAttempt] = useState(0)
  const [screenDims, setScreenDims] = useState<{ width: number; height: number } | undefined>()
  const [pipelineInfo, setPipelineInfo] = useState<DesktopPipelineInfo | undefined>()
  const [hostLockState, setHostLockState] = useState<HostLockState>('unknown')
  const [accessibilityMissing, setAccessibilityMissing] = useState(false)

  const wsRef = useRef<WebSocket | null>(null)
  const pcRef = useRef<RTCPeerConnection | null>(null)
  const ctrlDcRef = useRef<RTCDataChannel | null>(null)
  const videoRef = useRef<HTMLVideoElement | null>(null)
  // Phase-02 scaffold. Hidden <audio> sink for the BUNDLE'd Opus track once the
  // server adds it (gated on Windows kill-switch). Detached element — never
  // appended to DOM; `srcObject + play()` is enough for browser playback.
  const audioRef = useRef<HTMLAudioElement | null>(null)
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const attemptRef = useRef(0)
  const destroyedRef = useRef(false)
  const rvfcHandleRef = useRef<number | null>(null)
  // Distinguishes Cloudflare Quick Tunnel handshake transients (close/error
  // before onopen) from mid-session disconnects. Reset to false in connect();
  // flipped true once `ws.onopen` fires. Pairs with fastRetryUsedRef.
  const everOpenedThisCycleRef = useRef(false)
  const fastRetryUsedRef = useRef(false)

  // Latest UI tier — pushed into ctrl DC on open so the agent encodes at the
  // right bitrate from the first frame rather than falling back to default.
  const tierRef = useRef<QualityTier>(tier)
  useEffect(() => {
    tierRef.current = tier
  }, [tier])

  // Latest HiDPI preference — sent BEFORE the offer (not on ctrl DC) so the
  // agent builds the encoder at the right resolution from session-start.
  const hidpiRef = useRef<boolean>(hidpi)
  useEffect(() => {
    hidpiRef.current = hidpi
  }, [hidpi])

  // User's session-start audio intent (start unmuted vs muted). Sent inside
  // `capabilitiesClient.audio` so the agent picks the writer task's initial
  // mute state. Mid-session toggles never re-run this — the WS `audioToggle`
  // message flips a server-side atomic with no renegotiation.
  const audioRefBool = useRef<boolean>(audio)
  useEffect(() => {
    audioRefBool.current = audio
  }, [audio])

  // Infrastructure gate (operator setting + probe + pipeline). When true we
  // always add the recvonly audio transceiver to the offer so the agent has
  // a sendonly counterpart bound and can unmute mid-session by flipping its
  // writer atomic. Decoupled from `audioRefBool` so a user who opens muted
  // can still unmute instantly. Read once at session-start.
  const audioInfraRef = useRef<boolean>(audioInfra)
  useEffect(() => {
    audioInfraRef.current = audioInfra
  }, [audioInfra])

  // ── Hidden <video> element lifecycle ────────────────────────────────────
  //
  // Created once per hook; reused across reconnects. We never append it to
  // the DOM — assigning `srcObject` + calling `play()` is enough for the
  // decoder to run, and `requestVideoFrameCallback` still fires.
  useEffect(() => {
    const v = document.createElement('video')
    v.autoplay = true
    v.playsInline = true
    v.muted = true
    videoRef.current = v

    // Phase-02 scaffold sink. Stays silent until the server attaches an audio
    // transceiver (gated on Windows WASAPI kill-switch). The user-gesture for
    // autoplay is already satisfied by the session-start tap on the desktop
    // page (no `playsInline` — that's a video-only attribute).
    const a = document.createElement('audio')
    a.autoplay = true
    audioRef.current = a

    return () => {
      v.srcObject = null
      videoRef.current = null
      a.srcObject = null
      audioRef.current = null
    }
  }, [])

  // ── rVFC draw loop ──────────────────────────────────────────────────────
  //
  // `requestVideoFrameCallback` fires only when a new decoded frame is
  // available — no redraws on idle. Self-reschedules each call.
  const startFrameLoop = useCallback(() => {
    const video = videoRef.current
    if (!video) return
    // Some older browsers lack rVFC; gracefully fall back to rAF.
    if (typeof video.requestVideoFrameCallback !== 'function') {
      const loop = () => {
        if (destroyedRef.current) return
        onFrame(video, video)
        rvfcHandleRef.current = requestAnimationFrame(loop) as unknown as number
      }
      rvfcHandleRef.current = requestAnimationFrame(loop) as unknown as number
      return
    }
    const step = () => {
      if (destroyedRef.current) return
      onFrame(video, video)
      rvfcHandleRef.current = video.requestVideoFrameCallback(step) as unknown as number
    }
    rvfcHandleRef.current = video.requestVideoFrameCallback(step) as unknown as number
  }, [onFrame])

  const stopFrameLoop = useCallback(() => {
    const video = videoRef.current
    const handle = rvfcHandleRef.current
    if (handle === null) return
    if (video && typeof video.cancelVideoFrameCallback === 'function') {
      try {
        video.cancelVideoFrameCallback(handle)
      } catch {
        /* best effort */
      }
    } else {
      cancelAnimationFrame(handle)
    }
    rvfcHandleRef.current = null
  }, [])

  // ── Teardown / reconnect ────────────────────────────────────────────────
  const teardown = useCallback(() => {
    if (reconnectTimerRef.current) clearTimeout(reconnectTimerRef.current)
    stopFrameLoop()
    ctrlDcRef.current?.close()
    pcRef.current?.close()
    if (wsRef.current && wsRef.current.readyState < WebSocket.CLOSING) {
      wsRef.current.close()
    }
    ctrlDcRef.current = null
    pcRef.current = null
    wsRef.current = null
    if (videoRef.current) videoRef.current.srcObject = null
  }, [stopFrameLoop])

  // ── Ctrl DC helper ──────────────────────────────────────────────────────
  function sendCtrl(ev: DesktopInputEvent) {
    const payload = JSON.stringify(ev)
    if (ctrlDcRef.current?.readyState === 'open') {
      ctrlDcRef.current.send(payload)
    } else if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(payload)
    }
  }

  // ── Connect ─────────────────────────────────────────────────────────────
  const connect = useCallback(() => {
    if (destroyedRef.current) return
    teardown()
    everOpenedThisCycleRef.current = false

    setStatus('connecting')
    const protocols = wsProtocols()
    const url = wsUrl(deviceId, forcePipeline)
    const ws = protocols ? new WebSocket(url, protocols) : new WebSocket(url)
    ws.binaryType = 'arraybuffer'
    wsRef.current = ws

    const pc = new RTCPeerConnection(STUN_CONFIG)
    pcRef.current = pc

    // Recvonly video transceiver — phase 03 server expects this in the first
    // offer so it can add its TrackLocalStaticSample without renegotiation.
    pc.addTransceiver('video', { direction: 'recvonly' })

    // Recvonly audio transceiver. Added whenever the operator-side audio
    // infrastructure is ready (probe + DB setting + video pipeline), even
    // when the user opens this session muted. The agent's sendonly Opus
    // transceiver (added BEFORE set_remote_description) needs a matching
    // m-line; without it the agent's transceiver becomes orphaned and zero
    // RTP packets flow. Pre-binding the transceiver lets mid-session
    // `audioToggle` flip a server-side atomic instead of forcing a session
    // reconnect to renegotiate audio onto the PC.
    if (audioInfraRef.current) {
      pc.addTransceiver('audio', { direction: 'recvonly' })
    }

    // Ctrl DC: reliable ordered (same as JPEG path — input events must not be
    // reordered). Externally-negotiated with id=2 so the server's peer
    // creates a matching DC.
    const ctrlDc = pc.createDataChannel('ctrl', {
      ordered: true,
      negotiated: true,
      id: 2,
    })
    ctrlDcRef.current = ctrlDc

    pc.onicecandidate = (e) => {
      if (e.candidate && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ice', candidate: e.candidate.toJSON() }))
      }
    }

    // Incoming video track — attach to the hidden video element and start
    // the draw loop.
    pc.ontrack = (e) => {
      // Phase-02 scaffold: route audio to the hidden <audio> sink. The server
      // does not add an audio transceiver yet, so this branch is dormant
      // until the WASAPI kill-switch passes.
      if (e.track.kind === 'audio') {
        const audio = audioRef.current
        if (!audio) return
        const stream = e.streams[0] ?? new MediaStream([e.track])
        audio.srcObject = stream
        audio.play().catch(() => { /* iOS autoplay may reject — caller's gesture path retries */ })
        return
      }
      if (e.track.kind !== 'video') return
      const video = videoRef.current
      if (!video) return
      // Ask Chrome's WebRTC pipeline to play frames as soon as they decode,
      // skipping the default jitter buffer (which is tuned for media playback,
      // not interactive remote control). Chrome 92+; ignored elsewhere.
      // Cuts ~30–80 ms off glass-to-glass on LAN — the largest single win
      // identified in the phase-03 latency benchmark.
      const recvr = e.receiver as RTCRtpReceiver & { playoutDelayHint?: number }
      try { recvr.playoutDelayHint = 0 } catch { /* unsupported browser */ }
      const stream = e.streams[0] ?? new MediaStream([e.track])
      video.srcObject = stream
      // `play()` may reject on autoplay restrictions; we catch because the
      // track still dispatches frames for rVFC regardless.
      video.play().catch(() => {})
      setStatus('streaming')
      attemptRef.current = 0
      setAttempt(0)
      startFrameLoop()
    }

    ctrlDc.onopen = () => {
      try {
        ctrlDc.send(JSON.stringify({ t: 'quality', tier: tierRef.current }))
      } catch {
        /* DC could close between open and send */
      }
    }

    ws.onopen = () => {
      // If this WS is already stale (StrictMode cleanup closed our pc), skip.
      if (wsRef.current !== ws) return
      everOpenedThisCycleRef.current = true
      fastRetryUsedRef.current = false
      setStatus('signaling')
      // Announce decoder capabilities BEFORE sending the offer so the server
      // can decide H.264 vs JPEG before the first ICE candidate arrives.
      //
      // `loopback` tells the server its receiver is on the same machine.
      // The server uses this to bypass REMB → encoder bitrate updates:
      // Chrome's GCC collapses to ~5 kbps on loopback because it interprets
      // CPU-contention scheduling jitter as network congestion, killing
      // screen-share quality. Parsec/Rustdesk skip generic BWE for the
      // same reason. Detected by hostname rather than by ICE candidate
      // (we don't have post-ICE info pre-offer) so it's an honest hint,
      // not a guarantee — agent treats it as such.
      const isLoopback =
        location.hostname === 'localhost' ||
        location.hostname === '127.0.0.1' ||
        location.hostname === '[::1]' ||
        location.hostname === '::1'
      // Probe the actual createOffer SDP rtpmap for negotiable codecs
      // BEFORE advertising — `getCapabilities` can overstate WebRTC RTP
      // support on some browsers. The SDP probe matches what the agent
      // validates against.
      probeCodecSdpSupport()
        .then(() => {
          if (wsRef.current !== ws || ws.readyState !== WebSocket.OPEN) return
          // Advertise every codec the browser will actually emit in its
          // offer's rtpmap. Agent's `choose()` priority chain picks the
          // highest one both sides agree on (AV1 → VP9 → H.264 → JPEG).
          // Order in this list is irrelevant — agent reads it as a set.
          const clientCodecs: string[] = ['h264-baseline-3.1']
          if (supportsVp9Video()) clientCodecs.push('vp9')
          if (supportsAv1Video()) clientCodecs.push('av1')
          ws.send(
            JSON.stringify({
              type: 'capabilitiesClient',
              codecs: clientCodecs,
              webcodecs: false,
              audio: audioRefBool.current,
              loopback: isLoopback,
            }),
          )
          // Push the persisted HiDPI preference before the offer so the
          // encoder is built at the right resolution from session-start.
          // Skipping this would force a reconnect every time the user has
          // HiDPI on.
          ws.send(JSON.stringify({ type: 'settings', hidpi: hidpiRef.current }))
          // Push the quality tier too so the encoder picks up the user's
          // resolution scaling on first frame instead of building at the
          // default Med, only to be restarted by the first ctrl-DC tier
          // message. Mirrors the hidpi pre-offer hand-off.
          ws.send(JSON.stringify({ type: 'quality', tier: tierRef.current }))
          return pc.createOffer()
        })
        .then((offer) => {
          if (!offer) return
          return pc.setLocalDescription(offer)
        })
        .then(() => {
          if (ws.readyState === WebSocket.OPEN && pc.localDescription) {
            ws.send(JSON.stringify({ type: 'offer', sdp: pc.localDescription.sdp }))
          }
        })
        .catch(() => {
          // Stale PC from a previous StrictMode mount may reject after the
          // live mount replaced our refs — don't cascade its failure.
          if (pcRef.current !== pc) return
          // handleDisconnect is a hoisted `function` declared below; the
          // mutual reference between connect/handleDisconnect is intentional
          // and safe (no reassignment, function-declaration hoisting handles
          // it at runtime).
          // eslint-disable-next-line react-hooks/immutability
          handleDisconnect()
        })
    }

    ws.onmessage = (e: MessageEvent) => {
      if (wsRef.current !== ws) return
      if (typeof e.data !== 'string') return
      let msg: Record<string, unknown>
      try {
        msg = JSON.parse(e.data) as Record<string, unknown>
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
        case 'capabilities':
          if (msg.width && msg.height) {
            setScreenDims({ width: msg.width as number, height: msg.height as number })
          }
          break
        case 'pipelineChosen':
        case 'pipeline': {
          // Server's chosen pipeline + reason. `pipelineChosen` fires at
          // session start; `pipeline` fires at first IDR carrying avcC +
          // authoritative `hardware_accel`. We accept both so the pill can
          // render immediately on session start and refine on IDR.
          const rawMode = msg.mode
          const mode: 'h264' | 'vp9' | 'av1' | 'jpeg' =
            rawMode === 'h264'
              ? 'h264'
              : rawMode === 'vp9'
                ? 'vp9'
                : rawMode === 'av1'
                  ? 'av1'
                  : 'jpeg'
          const reason = typeof msg.reason === 'string' ? msg.reason : 'unknown'
          const hwRaw = (msg as Record<string, unknown>).hardwareAccel
            ?? (msg as Record<string, unknown>).hardware_accel
          const hardwareAccel =
            typeof hwRaw === 'boolean' ? hwRaw : undefined

          setPipelineInfo({ mode, reason, hardwareAccel } as DesktopPipelineInfo)

          if (mode === 'jpeg') {
            // Server picked JPEG even though the SPA mounted the H.264 hook
            // — we'd be reading audio out of a video transceiver. Tear down
            // and let the page re-mount as JPEG.
            console.warn('video-session: server selected JPEG; remounting')
            setForceJpeg(hostId)
            window.dispatchEvent(new CustomEvent(DESKTOP_FALLBACK_PENDING_EVENT))
            if (!destroyedRef.current) handleDisconnect()
          }
          break
        }
        case 'fallbackPending': {
          // Server's session-start IDR watchdog fired. Set the sessionStorage
          // marker so the next mount tells the agent to skip H.264, then
          // notify the page to re-mount as JPEG. Tear down our PC — the
          // server has already closed its side.
          const reason = typeof msg.reason === 'string' ? msg.reason : 'no-idr'
          console.warn('video-session: fallbackPending', reason)
          setForceJpeg(hostId)
          window.dispatchEvent(new CustomEvent(DESKTOP_FALLBACK_PENDING_EVENT))
          if (!destroyedRef.current) handleDisconnect()
          break
        }
        case 'hostLocked':
          setHostLockState('locked')
          break
        case 'hostUnlocked':
          setHostLockState('unlocked')
          // H.264 path: the agent's per-session unlock subscriber pushes a
          // PLI into the encoder's pli channel, so the next frame on the
          // wire is a fresh IDR. Nothing for the client to do beyond
          // clearing the overlay state.
          break
        case 'accessibilityMissing':
          setAccessibilityMissing(true)
          break
      }
    }

    ws.onerror = () => {
      if (wsRef.current !== ws) return
      handleDisconnect()
    }
    ws.onclose = () => {
      // StrictMode double-mount: a stale WS closing must not abort the live
      // replacement WS (see use-desktop-session.ts for the full race).
      if (wsRef.current !== ws) return
      if (!destroyedRef.current) handleDisconnect()
    }
  }, [deviceId, teardown, startFrameLoop]) // eslint-disable-line react-hooks/exhaustive-deps

  function handleDisconnect() {
    if (destroyedRef.current) return

    // Cloudflare Quick Tunnel handshake transients close the WS before
    // `onopen` ever fires. Burn one immediate retry per cycle without
    // bumping the attempt counter or flipping into the 'reconnecting' state.
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

  const sendInput = useCallback((ev: DesktopInputEvent) => sendCtrl(ev), [])
  const setQuality = useCallback((q: QualityTier) => sendCtrl({ t: 'quality', tier: q }), [])
  // Mid-session HiDPI flip → server tears down the PC; the reconnect path
  // re-runs ws.onopen which sends the new persisted hidpi before the offer.
  const setSettings = useCallback(
    (next: { hidpi: boolean }) => sendCtrl({ t: 'settings', hidpi: next.hidpi }),
    [],
  )
  // Audio toggle. Goes over the signaling WS only (not ctrl DC) — the WS
  // parses SignalIn (type-tagged) and knows `audioToggle`; the ctrl DC
  // parses WireInput (t-tagged) and would silently drop it. Bidirectional
  // and instant: the agent flips a shared `audio_muted` atomic the writer
  // task reads, with no PC renegotiation and no audio-pipeline teardown.
  // Track the latest intent locally so a flip that races a ws reconnect
  // gets re-applied on the next session via `capabilitiesClient.audio`.
  const toggleAudio = useCallback((enabled: boolean) => {
    audioRefBool.current = enabled
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: 'audioToggle', enabled }))
    }
  }, [])

  const disconnect = useCallback(() => {
    destroyedRef.current = true
    teardown()
    setStatus('disconnected')
  }, [teardown])

  useEffect(() => {
    if (!deviceId) return
    destroyedRef.current = false
    connect()
    return () => {
      destroyedRef.current = true
      teardown()
    }
  }, [hostId, deviceId]) // eslint-disable-line react-hooks/exhaustive-deps

  // When the supervisor rotates the tunnel URL, teardown and reconnect so the
  // WS upgrade uses the updated tunnel base within ~3s.
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

  return {
    status,
    sendInput,
    setQuality,
    setSettings,
    toggleAudio,
    disconnect,
    attempt,
    screenDims,
    pipelineInfo,
    hostLockState,
    accessibilityMissing,
  }
}
