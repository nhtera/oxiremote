// Remote desktop page — lazy-loaded route at /h/:hostId/desktop.
//
// Fetches capabilities first; unavailable state shows a helpful message.
// Branches between two pipeline views based on `preferred_pipeline` from
// the server plus client-side feature detection:
//   - H.264 over WebRTC video track — `DesktopH264View`
//   - JPEG tiles over DataChannel — `DesktopJpegView`
// Both views push session state + API up to this shell, which owns the
// shared toolbar, gesture help sheet, and reconnect modal.

import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import type {
  DesktopInputEvent,
  DesktopPipelineInfo,
  DesktopStatus,
  QualityTier,
} from '../hooks/use-desktop-session'
import { DESKTOP_FALLBACK_PENDING_EVENT } from '../hooks/use-desktop-session'
import type { InputMode, GestureMode } from '../hooks/use-desktop-input'
import { supportsH264Video } from '../hooks/use-desktop-video-session'
import PipelineStatusPill from '../components/pipeline-status-pill'
import DesktopJpegView from '../components/desktop-jpeg-view'
import DesktopH264View from '../components/desktop-h264-view'
import LockedOverlay from '../components/locked-overlay'
import type { DesktopGestureApi } from '../components/desktop-jpeg-view'
import DesktopToolbar from '../components/desktop-toolbar'
import DesktopTopStrip from '../components/desktop-top-strip'
import DesktopGestureHelp from '../components/desktop-gesture-help'
import DesktopOnscreenKeyboard from '../components/desktop-onscreen-keyboard'
import DesktopTextBatchSheet from '../components/desktop-text-batch-sheet'
import DesktopTextStrip from '../components/desktop-text-strip'
import DesktopExitConfirm from '../components/desktop-exit-confirm'
import ReconnectModal from '../components/reconnect-modal'
// Phase-03 step 6: stats overlay is lazy-loaded so the main bundle stays
// under the 250 KB cap. Mount only when `?stats=1` URL param OR persisted
// "Show stream stats" toggle is on. Suspense fallback is null — we'd
// rather render nothing than flash a placeholder for ~50 ms.
const DesktopStatsOverlay = lazy(() => import('../components/desktop-stats-overlay'))
import { useConfirm } from '../components/ui'
import { useSendTextLiteral } from '../hooks/use-desktop-input'
import { useDesktopKeyboardCapture } from '../hooks/use-desktop-keyboard-capture'

interface Capabilities {
  available: boolean
  quality_tiers: QualityTier[]
  monitors: { id: number; label: string; width: number; height: number }[]
  /** Operator preference: 'auto' (default) | 'h264' | 'jpeg'. Phase-01: the
   *  old binary surface was 'h264'|'jpeg'; new agents emit 'auto'. SPA treats
   *  'auto' as "try H.264 if the client supports it" — same effect as the old
   *  'h264' preference but without the fail-closed semantics. */
  preferred_pipeline?: 'auto' | 'jpeg' | 'h264'
  /** What `Auto` would resolve to right now given an optimistic client cap
   *  set. Used as the SPA's pre-mount hint when `preferred_pipeline === 'auto'`. */
  chosen_default?: 'jpeg' | 'h264'
  /** Phase-02a: operator has flipped the desktop_audio_enabled setting on. */
  audio_enabled?: boolean
  /** Phase-02a: build-side `desktop::audio::probe_supported()` outcome — only
   *  AND-merged with `audio_enabled` does the SPA advertise `audio: true` in
   *  the WS `capabilitiesClient`. */
  audio_supported?: boolean
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  /** Phase-02a: H.264 view exposes this; JPEG view doesn't (no audio path). */
  toggleAudio?: (enabled: boolean) => void
  disconnect: () => void
  /** Trigger a PNG screenshot download. View-specific implementation. */
  screenshot?: () => Promise<void>
}

const noopApi: SessionApi = {
  sendInput: () => {},
  setQuality: () => {},
  setSettings: () => {},
  disconnect: () => {},
}

interface DisplaySettings {
  hidpi: boolean
  smoothScaling: boolean
}

const SETTINGS_KEY = 'oxi.desktop.settings'
const DEFAULT_SETTINGS: DisplaySettings = { hidpi: false, smoothScaling: false }

// User-driven pipeline preference. Persisted globally (not per-host) since
// it tracks browser-side decode capability + bandwidth preference, which is
// generally machine-uniform. `'auto'` defers to the agent's chosen-default
// (which itself respects `OXI_VIDEO_PIPELINE`); `'h264'` and `'jpeg'` are
// session-scoped overrides that ride on the WS upgrade as
// `?force_pipeline=...`.
type PipelinePref = 'auto' | 'h264' | 'jpeg'
const PIPELINE_PREF_KEY = 'oxi.desktop.pipeline_pref'
function loadPipelinePref(): PipelinePref {
  if (typeof localStorage === 'undefined') return 'auto'
  const v = localStorage.getItem(PIPELINE_PREF_KEY)
  return v === 'h264' || v === 'jpeg' ? v : 'auto'
}

// Phase-03 step 7: stats overlay gate. URL `?stats=1` enables for the
// current session only (debug ergonomics — leave the URL, refresh, gone).
// Persisted setting `oxi.desktop.show_stats` enables across sessions.
// Either is sufficient to render the overlay.
const STATS_PREF_KEY = 'oxi.desktop.show_stats'
function urlWantsStats(): boolean {
  if (typeof window === 'undefined') return false
  return new URLSearchParams(window.location.search).get('stats') === '1'
}
function loadStatsPref(): boolean {
  if (typeof localStorage === 'undefined') return false
  return localStorage.getItem(STATS_PREF_KEY) === '1'
}

function loadSettings(): DisplaySettings {
  if (typeof localStorage === 'undefined') return DEFAULT_SETTINGS
  try {
    const raw = localStorage.getItem(SETTINGS_KEY)
    if (!raw) return DEFAULT_SETTINGS
    const parsed = JSON.parse(raw) as Partial<DisplaySettings>
    return {
      hidpi: typeof parsed.hidpi === 'boolean' ? parsed.hidpi : DEFAULT_SETTINGS.hidpi,
      smoothScaling:
        typeof parsed.smoothScaling === 'boolean'
          ? parsed.smoothScaling
          : DEFAULT_SETTINGS.smoothScaling,
    }
  } catch {
    return DEFAULT_SETTINGS
  }
}

export default function DesktopPage() {
  const { hostId = '' } = useParams<{ hostId: string }>()
  const navigate = useNavigate()
  const hostLabel = useHostStore((s) => s.label) ?? hostId.slice(0, 8) ?? 'host'
  const [caps, setCaps] = useState<Capabilities | null>(null)
  const [capsError, setCapsError] = useState('')
  const [deviceId, setDeviceId] = useState<string | null>(null)
  // Default to High on retina (sharper text + 2× framerate). Phase 01's idle
  // backoff + thumbnail short-circuit makes the higher framerate ~free at
  // rest; the bandwidth difference only shows up under motion.
  const [quality, setQuality] = useState<QualityTier>(() => {
    if (typeof window === 'undefined') return 'med'
    return (window.devicePixelRatio ?? 1) >= 1.5 ? 'high' : 'med'
  })
  const [inputMode, setInputMode] = useState<InputMode>('touch')
  const [gestureMode, setGestureMode] = useState<GestureMode>('pointer')
  const [showHelp, setShowHelp] = useState(false)
  const [showKeyboard, setShowKeyboard] = useState(false)
  // Mobile-only RVNC-style accessory bar (focus-catcher + ⌫/↵/▼). Floats
  // above the soft keyboard via visualViewport. Toggled from the top strip's
  // ⌨ icon. Distinct from `showKeyboard`, which opens the full modifier /
  // F-key / arrows sheet — accessible from inside the accessory.
  const [keyboardOpen, setKeyboardOpen] = useState(false)
  // Soft-keyboard occluded height in CSS px. iOS / Android keep the layout
  // viewport at full screen size and overlay the keyboard on top, which
  // would let the canvas paint into the area behind the keyboard. We pad
  // the outer container by this height so the flex-1 canvas wrap shrinks
  // to fit the area visible above the keyboard. Tracks visualViewport
  // independently of the composer (which has its own listener for the
  // bar's `bottom` offset) so the page remains responsive even if the
  // composer is closed by the user dismissing the keyboard via OS gesture.
  const [keyboardOffset, setKeyboardOffset] = useState(0)
  useEffect(() => {
    if (!keyboardOpen) {
      setKeyboardOffset(0)
      return
    }
    const vp = window.visualViewport
    if (!vp) return
    const update = () => {
      const obscured = Math.max(0, window.innerHeight - vp.height - vp.offsetTop)
      setKeyboardOffset(obscured)
    }
    vp.addEventListener('resize', update)
    vp.addEventListener('scroll', update)
    update()
    return () => {
      vp.removeEventListener('resize', update)
      vp.removeEventListener('scroll', update)
    }
  }, [keyboardOpen])
  const [showTextBatch, setShowTextBatch] = useState(false)
  const [showExitConfirm, setShowExitConfirm] = useState(false)
  // Forces a remount of the active video view — used by the mobile Reload
  // button so the operator can recover from a stuck stream without leaving
  // the page (browser-back would lose any partial gesture / tool state).
  const [reloadNonce, setReloadNonce] = useState(0)
  const [inFullscreen, setInFullscreen] = useState(false)
  // HiDPI + smooth-scaling toggles, persisted per-device.
  const [settings, setSettingsState] = useState<DisplaySettings>(() => loadSettings())
  useEffect(() => {
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings))
    } catch {
      /* private mode / quota exceeded — best-effort */
    }
  }, [settings])

  // Session state pushed up from the mounted view.
  const [status, setStatus] = useState<DesktopStatus>('idle')
  const [attempt, setAttempt] = useState(0)
  const [screenDims, setScreenDims] = useState<{ width: number; height: number } | undefined>(
    undefined,
  )
  const [pipelineInfo, setPipelineInfo] = useState<DesktopPipelineInfo | undefined>()
  const [hostLockState, setHostLockState] = useState<'unknown' | 'locked' | 'unlocked'>('unknown')
  const [accessibilityMissing, setAccessibilityMissing] = useState(false)
  // Per-session force-JPEG flag — flipped true when the H.264 hook reports
  // `fallbackPending`. Causes the next render to mount the JPEG view; the
  // JPEG hook then appends `?force_pipeline=jpeg` to its WS upgrade so the
  // agent stays on JPEG for this session. Reset on `Reload` so the user can
  // retry H.264 deliberately.
  const [forceJpegSession, setForceJpegSession] = useState(false)
  const sessionApiRef = useRef<SessionApi>(noopApi)

  // Phase-03 step 7: stats overlay gate. Initial value composes the URL
  // hint with the persisted setting; further changes only flow through the
  // popover toggle (URL is read-once at mount).
  const [showStats, setShowStatsState] = useState<boolean>(
    () => urlWantsStats() || loadStatsPref(),
  )
  const setShowStats = useCallback((next: boolean) => {
    setShowStatsState(next)
    try {
      if (next) localStorage.setItem(STATS_PREF_KEY, '1')
      else localStorage.removeItem(STATS_PREF_KEY)
    } catch {
      /* private mode / quota — best-effort */
    }
  }, [])

  // User-driven pipeline preference (Auto/H.264/JPEG). Mid-session change
  // triggers a reconnect via `reloadNonce` so the WS upgrade picks up the
  // new `?force_pipeline=` query param. `setForceJpegSession(false)` clears
  // any prior fallback flag so user intent always wins.
  const [pipelinePref, setPipelinePrefState] = useState<PipelinePref>(() => loadPipelinePref())
  const setPipelinePref = useCallback((p: PipelinePref) => {
    setPipelinePrefState((prev) => {
      if (prev === p) return prev
      try {
        localStorage.setItem(PIPELINE_PREF_KEY, p)
      } catch {
        /* private mode / quota — best-effort */
      }
      sessionApiRef.current.disconnect()
      setForceJpegSession(false)
      setReloadNonce((n) => n + 1)
      return p
    })
  }, [])

  // Phase-02a: user audio intent. `null` = follow gate default (audio on when
  // the gate passes); `true` = user explicitly on; `false` = user explicitly
  // off (sticky across reconnects). The actual `audioActive` flag is derived
  // below from intent ∧ live gate so a pipeline switch doesn't clobber the
  // operator's mute choice.
  const [audioPref, setAudioPref] = useState<boolean | null>(null)
  // Bidirectional. On→Off: send `audioToggle=false` over the WS, server
  // tears the audio pipeline down via `UserToggleOff` (no video disruption).
  // Off→On: trigger a session reconnect — the SCK audio capture + Opus
  // transceiver are bound at session-start and webrtc-rs has no clean
  // mid-session "re-add audio" path, so we recycle the session.
  const handleToggleAudio = useCallback(() => {
    setAudioPref((cur) => {
      if (cur === false) {
        // Off → On: needs reconnect to add the audio transceiver back.
        sessionApiRef.current.disconnect()
        setForceJpegSession(false)
        setReloadNonce((n) => n + 1)
        return true
      }
      // On → Off: instant mute via WS — server tears audio without renegotiating.
      sessionApiRef.current.toggleAudio?.(false)
      return false
    })
  }, [])

  // Container that wraps the canvas. Tracked via ResizeObserver so we can
  // surface the displayed-vs-native scale as a zoom % in the mobile top strip.
  const canvasWrapRef = useRef<HTMLDivElement | null>(null)
  const [containerSize, setContainerSize] = useState<{ width: number; height: number } | null>(null)
  useEffect(() => {
    const el = canvasWrapRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect
      if (r) setContainerSize({ width: r.width, height: r.height })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Pinch-zoom user scale. Updated by useCanvasGestures via onZoomChange so
  // the indicator below reflects the *effective* on-screen scale, not just
  // the static letterbox-fit ratio.
  const [userScale, setUserScale] = useState(1)
  // Stable handle for the pinch/pan reset. Both views push their handle into
  // this ref via onGestureApi; the overflow menu's "Fit" item calls it.
  const gestureApiRef = useRef<DesktopGestureApi | null>(null)
  const handleGestureApi = useCallback((api: DesktopGestureApi) => {
    gestureApiRef.current = api
  }, [])
  const handleResetZoom = useCallback(() => {
    gestureApiRef.current?.resetZoom()
    setUserScale(1)
  }, [])

  // canvas uses `object-contain`, so the base scale is min(W/sw, H/sh).
  // Multiply by the user's pinch scale so the zoom % chip in the top strip
  // shows what the user actually sees on screen.
  const zoom =
    screenDims && containerSize && screenDims.width > 0 && screenDims.height > 0
      ? Math.min(
          containerSize.width / screenDims.width,
          containerSize.height / screenDims.height,
        ) * userScale
      : undefined

  // Resolve authenticated device_id from /api/me. Agent binds each WS to
  // the session's device_id; mismatch → 403. See phase-04 review C-1.
  useEffect(() => {
    fetch('/api/me', { credentials: 'include' })
      .then((r) => (r.ok ? (r.json() as Promise<{ device_id: string }>) : null))
      .then((data) => {
        if (data?.device_id) setDeviceId(data.device_id)
      })
      .catch(() => {})
  }, [])

  useEffect(() => {
    if (!hostId) return
    fetch(`/api/hosts/${hostId}/desktop/capabilities`, { credentials: 'include' })
      .then((r) => {
        if (!r.ok) throw new Error(`${r.status}`)
        return r.json() as Promise<Capabilities>
      })
      .then(setCaps)
      .catch((e: unknown) => setCapsError(String(e)))
  }, [hostId])

  const onSessionChange = useCallback(
    (s: {
      status: DesktopStatus
      attempt: number
      screenDims?: { width: number; height: number }
      pipelineInfo?: DesktopPipelineInfo
      hostLockState?: 'unknown' | 'locked' | 'unlocked'
      accessibilityMissing?: boolean
    }) => {
      setStatus(s.status)
      setAttempt(s.attempt)
      if (s.screenDims) setScreenDims(s.screenDims)
      if (s.pipelineInfo) setPipelineInfo(s.pipelineInfo)
      if (s.hostLockState) setHostLockState(s.hostLockState)
      if (s.accessibilityMissing) setAccessibilityMissing(true)
    },
    [],
  )

  // Listen for the H.264 hook's `fallbackPending` signal: re-mount as JPEG
  // and bump the reload nonce so the new view fully reinitialises (vs
  // re-using a torn-down PC). The H.264 hook has already set the
  // sessionStorage marker the JPEG hook consumes on its next connect.
  useEffect(() => {
    function onFallback() {
      setForceJpegSession(true)
      setReloadNonce((n) => n + 1)
      setPipelineInfo(undefined) // pill clears until the JPEG hook emits its own
    }
    window.addEventListener(DESKTOP_FALLBACK_PENDING_EVENT, onFallback)
    return () =>
      window.removeEventListener(DESKTOP_FALLBACK_PENDING_EVENT, onFallback)
  }, [])
  const onSessionApi = useCallback((api: SessionApi) => {
    sessionApiRef.current = api
  }, [])

  const handleQualityChange = useCallback((tier: QualityTier) => {
    setQuality(tier)
    sessionApiRef.current.setQuality(tier)
  }, [])
  const sendInput = useCallback((ev: DesktopInputEvent) => {
    sessionApiRef.current.sendInput(ev)
  }, [])
  // Reconnect-modal Exit: tear down the session AND leave the page. Just
  // calling disconnect() is a no-op in the exhausted branch — status is
  // already 'disconnected' and attempt stays >= 3, so the modal's open
  // predicate (showReconnect) never flips and the user gets stuck.
  const handleReconnectExit = useCallback(() => {
    sessionApiRef.current.disconnect()
    navigate(`/h/${hostId}`)
  }, [hostId, navigate])

  const confirm = useConfirm()

  const handleExit = useCallback(async () => {
    const ok = await confirm({
      title: 'Exit remote desktop?',
      message: 'The current session will end. You can rejoin from the host page.',
      confirmText: 'Exit',
      danger: true,
    })
    if (!ok) return
    sessionApiRef.current.disconnect()
    navigate(`/h/${hostId}`)
  }, [confirm, hostId, navigate])

  // Mobile back-gesture / browser back interception.
  // On mobile (pointer: coarse), we push a dummy history entry when a session
  // is active so the OS back gesture pops it back to our dummy rather than
  // navigating away. We then show the exit-confirm modal instead.
  // On PC (pointer: fine) we skip this — no native back gesture in the browser.
  const isMobile =
    typeof window !== 'undefined' && window.matchMedia('(pointer: coarse)').matches

  useEffect(() => {
    if (!isMobile) return
    // Push a dummy state so the next back gesture hits popstate before leaving.
    history.pushState({ desktopBackGuard: true }, '')

    function onPopState(_e: PopStateEvent) {
      // The dummy guard entry just popped. Re-push it so the next back gesture
      // also fires popstate, and show the exit-confirm modal now.
      history.pushState({ desktopBackGuard: true }, '')
      setShowExitConfirm(true)
    }

    // Listen on the window so we catch the popstate from the guard entry above.
    window.addEventListener('popstate', onPopState)
    return () => {
      window.removeEventListener('popstate', onPopState)
    }
  }, [isMobile])

  // Confirm exit from the mobile exit-confirm modal.
  const handleExitConfirm = useCallback(() => {
    setShowExitConfirm(false)
    sessionApiRef.current.disconnect()
    navigate(`/h/${hostId}`)
  }, [hostId, navigate])

  // Text composer send — one `{t:'text'}` ctrl frame per call (chunked at
  // 4 KB UTF-8 to stay below the server-side cap). Replaces the old per-char
  // keycode synthesis so punctuation and Unicode land verbatim on the host.
  const handleSendTextLiteral = useSendTextLiteral(sendInput)

  // Desktop physical-keyboard capture. While a stream is up and no form /
  // button is focused, every keystroke is forwarded to the host — standard
  // VNC behavior. Skips while overlays own focus so users can still interact
  // with settings / sheets / the mobile accessory's input.
  const captureEnabled = status === 'streaming' || status === 'fallback'
  useDesktopKeyboardCapture(captureEnabled, sendInput)

  const handleReload = useCallback(() => {
    sessionApiRef.current.disconnect()
    // User-driven reload: give H.264 another chance. The sessionStorage
    // marker was consumed when the JPEG hook ran, so this just clears the
    // page-level flag that pinned the view to JPEG.
    setForceJpegSession(false)
    setReloadNonce((n) => n + 1)
  }, [])

  const handleScreenshot = useCallback(() => {
    void sessionApiRef.current.screenshot?.().catch(() => {
      // The view either wasn't ready or the browser denied the capture path.
      // Surface nothing — the next try will succeed once a frame has landed.
    })
  }, [])

  const handleFullscreen = useCallback(async () => {
    type FsElement = HTMLElement & {
      webkitRequestFullscreen?: () => Promise<void> | void
    }
    type FsDocument = Document & {
      webkitFullscreenElement?: Element | null
      webkitExitFullscreen?: () => Promise<void> | void
    }
    const doc = document as FsDocument
    const isOn = !!(document.fullscreenElement || doc.webkitFullscreenElement)
    try {
      if (isOn) {
        if (document.exitFullscreen) await document.exitFullscreen()
        else if (doc.webkitExitFullscreen) await doc.webkitExitFullscreen()
      } else {
        const el = document.documentElement as FsElement
        if (el.requestFullscreen) await el.requestFullscreen()
        else if (el.webkitRequestFullscreen) await el.webkitRequestFullscreen()
      }
    } catch {
      // Permission denied / API missing — silently no-op so a misclick on
      // unsupported iOS Safari doesn't surface an error toast.
    }
  }, [])

  useEffect(() => {
    const onChange = () => {
      type FsDocument = Document & { webkitFullscreenElement?: Element | null }
      const doc = document as FsDocument
      setInFullscreen(!!(document.fullscreenElement || doc.webkitFullscreenElement))
    }
    document.addEventListener('fullscreenchange', onChange)
    document.addEventListener('webkitfullscreenchange', onChange)
    return () => {
      document.removeEventListener('fullscreenchange', onChange)
      document.removeEventListener('webkitfullscreenchange', onChange)
    }
  }, [])

  // Settings change handler. HiDPI flips trigger a brief session reconnect on
  // the H.264 path (encoder dims fixed at init) — confirm before sending so
  // the user isn't surprised by a freeze. Smooth-scaling is render-only.
  const handleSettingsChange = useCallback(
    async (next: { hidpi: boolean; smoothScaling: boolean }) => {
      const hidpiChanged = next.hidpi !== settings.hidpi
      if (hidpiChanged) {
        const ok = await confirm({
          title: 'Switch High-DPI mode?',
          message: 'The session will briefly reconnect.',
          confirmText: 'Continue',
        })
        if (!ok) {
          // Keep smoothScaling change even if user cancels HiDPI.
          if (next.smoothScaling !== settings.smoothScaling) {
            setSettingsState({ hidpi: settings.hidpi, smoothScaling: next.smoothScaling })
          }
          return
        }
      }
      setSettingsState(next)
      if (hidpiChanged) {
        sessionApiRef.current.setSettings({ hidpi: next.hidpi })
      }
    },
    [confirm, settings.hidpi, settings.smoothScaling],
  )

  // Unavailable / loading states
  if (capsError) {
    return (
      <div className="flex items-center justify-center h-full p-6">
        <div className="text-text-muted text-sm">Failed to load desktop capabilities.</div>
      </div>
    )
  }
  if (caps && !caps.available) {
    return (
      <div className="flex items-center justify-center h-full p-6">
        <div className="max-w-sm text-center space-y-2">
          <div className="text-text-primary font-medium">Remote Desktop not available</div>
          <div className="text-text-muted text-sm">
            Enable Screen Recording permission in System Settings, then restart the agent.
          </div>
        </div>
      </div>
    )
  }
  if (!caps || !deviceId) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        Loading…
      </div>
    )
  }

  // Pick H.264 when: client can decode AND (user picked h264 OR (auto AND
  // operator/agent default resolves to H.264)) AND no per-session fallback.
  // User pref takes precedence over auto-resolution; on `pipelinePref='jpeg'`
  // we mount JPEG view + ride `?force_pipeline=jpeg` over the WS so the
  // server bypasses H.264 negotiation entirely.
  const wantsH264 =
    pipelinePref === 'h264' ||
    (pipelinePref === 'auto' &&
      (caps.preferred_pipeline === 'h264' ||
        (caps.preferred_pipeline === 'auto' && caps.chosen_default === 'h264')))
  const useH264 =
    pipelinePref !== 'jpeg' && wantsH264 && supportsH264Video() && !forceJpegSession
  // Wire query param sent to the agent. Must reflect the *actually mounted*
  // view, not just `pipelinePref` — when the H.264 watchdog flips us to JPEG
  // mid-session (forceJpegSession) but the user's pref is still 'h264',
  // sending `?force_pipeline=h264` from the JPEG view would force the agent
  // onto H.264 with a JPEG-shaped client offer (no video transceiver) and
  // close the WS at SDP merge. Rule: H.264 view forces 'h264' only on
  // explicit pick; JPEG view forces 'jpeg' on explicit pick OR fallback.
  const forcePipelineWire: 'h264' | 'jpeg' | undefined = useH264
    ? pipelinePref === 'h264'
      ? 'h264'
      : undefined
    : forceJpegSession || pipelinePref === 'jpeg'
      ? 'jpeg'
      : undefined
  const monitorDefault = caps.monitors[0]
    ? { width: caps.monitors[0].width, height: caps.monitors[0].height }
    : undefined
  // Live audio gate (caps + chosen pipeline) AND user intent. `audioPref`
  // lives above early returns; the gate is computed here so it can read the
  // post-early-return `useH264`. Computing the active flag inline (no extra
  // useState) means a pipeline switch re-derives without a stale reset.
  const audioGatePass = !!(caps?.audio_enabled && caps?.audio_supported) && useH264
  const audioActive = audioGatePass && audioPref !== false

  const showReconnect =
    status === 'reconnecting' || (status === 'disconnected' && attempt >= 3)

  return (
    <div
      className="relative flex flex-col lg:flex-row w-full h-full overflow-hidden bg-black"
      style={{ paddingBottom: keyboardOffset }}
    >
      <DesktopTopStrip
        onExit={handleExit}
        onReload={handleReload}
        onFullscreen={handleFullscreen}
        onScreenshot={handleScreenshot}
        inFullscreen={inFullscreen}
        zoom={zoom}
        inputMode={inputMode}
        onInputModeToggle={() => setInputMode((m) => (m === 'touch' ? 'trackpad' : 'touch'))}
        onShowKeyboard={() => setKeyboardOpen(true)}
        onShowTextBatch={() => setShowTextBatch(true)}
        onResetZoom={handleResetZoom}
        onShowGestureHelp={() => setShowHelp(true)}
        gestureMode={gestureMode}
        onGestureModeToggle={() => setGestureMode((g) => (g === 'pointer' ? 'rect' : 'pointer'))}
        hidpi={settings.hidpi}
        smoothScaling={settings.smoothScaling}
        quality={quality}
        onQualityChange={handleQualityChange}
        onSettingsChange={handleSettingsChange}
        pipeline={useH264 ? 'h264' : 'jpeg'}
        audioGated={audioGatePass}
        audioActive={audioActive}
        onToggleAudio={handleToggleAudio}
        pipelinePref={pipelinePref}
        onPipelinePrefChange={setPipelinePref}
        showStats={showStats}
        onShowStatsChange={setShowStats}
      />

      {/* Phase-03 step 6: stats overlay. Lazy-loaded, mounts only when
          gated on. The H.264 view is the only producer right now — JPEG
          sessions get a 404 and the overlay renders "stats: closed". */}
      {showStats && useH264 && (
        <Suspense fallback={null}>
          <DesktopStatsOverlay hostId={hostId} />
        </Suspense>
      )}

      {/* Plan 260512-2314: yellow accessibility banner when the agent reports
          OS Accessibility permission is missing. Sits above the canvas (not
          inside it) so it doesn't intercept canvas pointer events. */}
      {accessibilityMissing && (
        <div className="px-3 py-2 text-sm bg-yellow-500/10 border-b border-yellow-500/30 text-yellow-200">
          ⚠ Keyboard / mouse input may be unreliable. Grant Accessibility permission:{' '}
          <a
            href="x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            className="underline"
          >
            Open Settings
          </a>
        </div>
      )}

      {/* Disable touch/pointer events on the canvas while text-batch sheet is open
          so taps on the visible canvas don't register as remote clicks. */}
      <div ref={canvasWrapRef} className={`flex-1 relative min-h-0${showTextBatch ? ' pointer-events-none' : ''}`}>
        {/* Phase-01 pipeline-status pill: shows `H.264 (HW/SW)` or `JPEG`
            with a tooltip explaining the chosen-reason. `pointer-events-none`
            so it never steals canvas taps. Mounted in the canvas wrap so it
            scales correctly on both mobile + lg layouts without toolbar
            prop drilling. */}
        {pipelineInfo && (
          <div className="absolute top-2 right-2 z-10 pointer-events-none">
            <PipelineStatusPill
              mode={pipelineInfo.mode}
              reason={pipelineInfo.reason}
              hardwareAccel={pipelineInfo.hardwareAccel}
            />
          </div>
        )}
        {/* Plan 260512-2314: locked-screen overlay. Renders above the video
            element via absolute inset-0 + z-20 (pipeline pill is z-10). On
            unlock the agent forces an IDR, so the next frame on the wire is
            clean — the SPA just clears `hostLockState`. */}
        {hostLockState === 'locked' && <LockedOverlay />}
        {useH264 ? (
          <DesktopH264View
            key={`h264-${reloadNonce}`}
            hostId={hostId}
            deviceId={deviceId}
            quality={quality}
            inputMode={inputMode}
            gestureMode={gestureMode}
            hidpi={settings.hidpi}
            smoothScaling={settings.smoothScaling}
            monitorDefault={monitorDefault}
            hostLabel={hostLabel}
            onSessionChange={onSessionChange}
            onSessionApi={onSessionApi}
            onZoomChange={setUserScale}
            onGestureApi={handleGestureApi}
            bottomAnchor={keyboardOpen}
            audio={audioActive}
            forcePipeline={forcePipelineWire}
          />
        ) : (
          <DesktopJpegView
            key={`jpeg-${reloadNonce}`}
            hostId={hostId}
            deviceId={deviceId}
            quality={quality}
            inputMode={inputMode}
            gestureMode={gestureMode}
            hidpi={settings.hidpi}
            smoothScaling={settings.smoothScaling}
            monitorDefault={monitorDefault}
            hostLabel={hostLabel}
            onSessionChange={onSessionChange}
            onSessionApi={onSessionApi}
            onZoomChange={setUserScale}
            onGestureApi={handleGestureApi}
            bottomAnchor={keyboardOpen}
            forcePipeline={forcePipelineWire}
          />
        )}
      </div>

      {/* Mobile keyboard accessory — RVNC-style. Mounts only when the
          operator taps the ⌨ icon in the top strip, rides above the soft
          keyboard via visualViewport, and unmounts when the keyboard goes
          down. Canvas keeps the full screen the rest of the time. The
          accessory's "open keyboard sheet" button surfaces the existing
          modifier / arrow / F-key sheet. */}
      <DesktopTextStrip
        open={keyboardOpen}
        onClose={() => setKeyboardOpen(false)}
        onSend={handleSendTextLiteral}
        onKey={sendInput}
        onOpenSheet={() => setShowKeyboard(true)}
      />

      {/* Desktop sidebar (lg+) — keeps the full toolbar with modifier grid,
          input mode segmented control, and inline display popover. The
          mobile path uses the redesigned top strip + composer + sheet. */}
      <div className="hidden lg:block lg:w-60 lg:h-full lg:shrink-0">
        <DesktopToolbar
          quality={quality}
          onQualityChange={handleQualityChange}
          inputMode={inputMode}
          onInputModeToggle={() => setInputMode((m) => (m === 'touch' ? 'trackpad' : 'touch'))}
          gestureMode={gestureMode}
          onGestureModeToggle={() =>
            setGestureMode((g) => (g === 'pointer' ? 'rect' : 'pointer'))
          }
          onKeyEvent={sendInput}
          onShowGestureHelp={() => setShowHelp(true)}
          onShowOnscreenKeyboard={() => setShowKeyboard(true)}
          hidpi={settings.hidpi}
          smoothScaling={settings.smoothScaling}
          onSettingsChange={handleSettingsChange}
          pipeline={useH264 ? 'h264' : 'jpeg'}
          onExit={handleExit}
          textBatchOpen={showTextBatch}
          onToggleTextBatch={() => setShowTextBatch((v) => !v)}
          audioGated={audioGatePass}
          audioActive={audioActive}
          onToggleAudio={handleToggleAudio}
          pipelinePref={pipelinePref}
          onPipelinePrefChange={setPipelinePref}
          showStats={showStats}
          onShowStatsChange={setShowStats}
        />
      </div>

      <DesktopGestureHelp open={showHelp} onClose={() => setShowHelp(false)} />
      <DesktopOnscreenKeyboard
        open={showKeyboard}
        onClose={() => setShowKeyboard(false)}
        onKeyEvent={sendInput}
      />

      {/* Text-batch sheet — canvas pointer events disabled while open so
          accidental taps on the visible canvas don't register as clicks. */}
      <DesktopTextBatchSheet
        open={showTextBatch}
        onClose={() => setShowTextBatch(false)}
        onSend={handleSendTextLiteral}
      />

      <DesktopExitConfirm
        open={showExitConfirm}
        onCancel={() => setShowExitConfirm(false)}
        onExit={handleExitConfirm}
      />

      <ReconnectModal
        open={showReconnect}
        attempt={attempt}
        maxAttempts={3}
        exhausted={status === 'disconnected'}
        onCancel={handleReconnectExit}
        onRetry={status !== 'disconnected' ? handleReload : undefined}
      />
    </div>
  )
}
