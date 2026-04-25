// Remote desktop page — lazy-loaded route at /h/:hostId/desktop.
//
// Fetches capabilities first; unavailable state shows a helpful message.
// Branches between two pipeline views based on `preferred_pipeline` from
// the server plus client-side feature detection:
//   - H.264 over WebRTC video track (Phase 03) — `DesktopH264View`
//   - JPEG tiles over DataChannel (Phase 02) — `DesktopJpegView`
// Both views push session state + API up to this shell, which owns the
// shared toolbar, gesture help sheet, and reconnect modal.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useParams } from 'react-router-dom'
import type {
  DesktopInputEvent,
  DesktopStatus,
  QualityTier,
} from '../hooks/use-desktop-session'
import type { InputMode } from '../hooks/use-desktop-input'
import { supportsH264Video } from '../hooks/use-desktop-video-session'
import DesktopJpegView from '../components/desktop-jpeg-view'
import DesktopH264View from '../components/desktop-h264-view'
import DesktopToolbar from '../components/desktop-toolbar'
import DesktopGestureHelp from '../components/desktop-gesture-help'
import DesktopOnscreenKeyboard from '../components/desktop-onscreen-keyboard'
import ReconnectModal from '../components/reconnect-modal'
import { useConfirm } from '../components/ui'

interface Capabilities {
  available: boolean
  quality_tiers: QualityTier[]
  monitors: { id: number; label: string; width: number; height: number }[]
  /** Server operator preference — 'h264' requires a client that also supports it. */
  preferred_pipeline?: 'jpeg' | 'h264'
}

interface SessionApi {
  sendInput: (ev: DesktopInputEvent) => void
  setQuality: (tier: QualityTier) => void
  setSettings: (next: { hidpi: boolean }) => void
  disconnect: () => void
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
  const [caps, setCaps] = useState<Capabilities | null>(null)
  const [capsError, setCapsError] = useState('')
  const [deviceId, setDeviceId] = useState<string | null>(null)
  const [quality, setQuality] = useState<QualityTier>('med')
  const [inputMode, setInputMode] = useState<InputMode>('touch')
  const [showHelp, setShowHelp] = useState(false)
  const [showKeyboard, setShowKeyboard] = useState(false)
  // Phase 04 — HiDPI + smooth-scaling toggles, persisted per-device.
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
  const sessionApiRef = useRef<SessionApi>(noopApi)

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
    (s: { status: DesktopStatus; attempt: number }) => {
      setStatus(s.status)
      setAttempt(s.attempt)
    },
    [],
  )
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
  const disconnect = useCallback(() => {
    sessionApiRef.current.disconnect()
  }, [])

  const confirm = useConfirm()
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

  const useH264 = caps.preferred_pipeline === 'h264' && supportsH264Video()
  const monitorDefault = caps.monitors[0]
    ? { width: caps.monitors[0].width, height: caps.monitors[0].height }
    : undefined

  const showReconnect =
    status === 'reconnecting' || (status === 'disconnected' && attempt >= 3)

  return (
    <div className="relative flex flex-col lg:flex-row w-full h-full overflow-hidden bg-black">
      <div className="flex-1 relative min-h-0">
        {useH264 ? (
          <DesktopH264View
            hostId={hostId}
            deviceId={deviceId}
            quality={quality}
            inputMode={inputMode}
            hidpi={settings.hidpi}
            smoothScaling={settings.smoothScaling}
            monitorDefault={monitorDefault}
            onSessionChange={onSessionChange}
            onSessionApi={onSessionApi}
          />
        ) : (
          <DesktopJpegView
            hostId={hostId}
            deviceId={deviceId}
            quality={quality}
            inputMode={inputMode}
            hidpi={settings.hidpi}
            smoothScaling={settings.smoothScaling}
            monitorDefault={monitorDefault}
            onSessionChange={onSessionChange}
            onSessionApi={onSessionApi}
          />
        )}
      </div>

      <div className="fixed bottom-0 left-0 right-0 z-20 lg:static lg:w-60 lg:h-full lg:flex-shrink-0">
        <DesktopToolbar
          quality={quality}
          onQualityChange={handleQualityChange}
          inputMode={inputMode}
          onInputModeToggle={() => setInputMode((m) => (m === 'touch' ? 'trackpad' : 'touch'))}
          onKeyEvent={sendInput}
          onShowGestureHelp={() => setShowHelp(true)}
          onShowOnscreenKeyboard={() => setShowKeyboard(true)}
          hidpi={settings.hidpi}
          smoothScaling={settings.smoothScaling}
          onSettingsChange={handleSettingsChange}
          pipeline={useH264 ? 'h264' : 'jpeg'}
        />
      </div>

      <DesktopGestureHelp open={showHelp} onClose={() => setShowHelp(false)} />
      <DesktopOnscreenKeyboard
        open={showKeyboard}
        onClose={() => setShowKeyboard(false)}
        onKeyEvent={sendInput}
      />

      <ReconnectModal
        open={showReconnect}
        attempt={attempt}
        maxAttempts={3}
        exhausted={status === 'disconnected'}
        onCancel={disconnect}
      />
    </div>
  )
}
