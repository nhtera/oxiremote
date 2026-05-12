// Remote desktop toolbar — quality selector + 8-button mobile key row.
// Sticky modifiers (Ctrl/Alt/Shift/⌘): tap to toggle (accent highlight);
// the modifier is fused into the next key event then automatically cleared.
// On viewport ≥1024px this sits in a right sidebar; below that it's fixed bottom.

import { useEffect, useRef, useState } from 'react'
import type { QualityTier, DesktopInputEvent } from '../hooks/use-desktop-session'
import type { InputMode, GestureMode } from '../hooks/use-desktop-input'
import DesktopSettingsPopover from './desktop-settings-popover'
import TransportPill from './transport-pill'

interface Props {
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  inputMode: InputMode
  onInputModeToggle: () => void
  /** Higher-level gesture mode — 'pointer' (normal) vs 'rect' (marquee select). */
  gestureMode?: GestureMode
  onGestureModeToggle?: () => void
  onKeyEvent: (ev: DesktopInputEvent) => void
  onShowGestureHelp: () => void
  /** Optional FAB action — e.g. open the on-screen keyboard sheet. */
  onShowOnscreenKeyboard?: () => void
  hidpi: boolean
  smoothScaling: boolean
  onSettingsChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
  /** Active video pipeline. Surfaced as a chip so the user knows which path is in use. */
  pipeline: 'h264' | 'jpeg'
  /** Operator-initiated session end. Triggers a confirm dialog up in the page. */
  onExit?: () => void
  /** Whether the text-batch sheet is currently open. */
  textBatchOpen?: boolean
  /** Toggle the text-batch sheet. */
  onToggleTextBatch?: () => void
  /** Phase-02a: whether the audio gate passed at session-start. Drives row
   *  visibility — hidden entirely when no audio path exists for this session. */
  audioGated?: boolean
  /** True when the audio pipeline is alive on the wire right now. Flips
   *  false after `onToggleAudio` fires; cannot flip back without reconnect. */
  audioActive?: boolean
  /** Sidebar mute action — fires the per-session `audioToggle` over the WS,
   *  triggering server-side teardown with `UserToggleOff`. Re-enable requires
   *  reconnect (matches hidpi-flip / H.264 fallback policy). */
  onToggleAudio?: () => void
  /** User-driven pipeline preference — forwarded into the Display popover. */
  pipelinePref?: 'auto' | 'h264' | 'jpeg'
  onPipelinePrefChange?: (p: 'auto' | 'h264' | 'jpeg') => void
  /** Phase-03 step 7: persisted "Show stream stats" checkbox in the Display
   *  popover. Drives the lazy-loaded stats overlay in `desktop-page.tsx`. */
  showStats?: boolean
  onShowStatsChange?: (v: boolean) => void
}

// Sticky modifier keys — toggled on tap, cleared after next key dispatch
type ModKey = 'ctrl' | 'alt' | 'shift' | 'meta'

interface KeyDef {
  label: string
  code: string
  mod?: ModKey
  /** Pre-fused modifier(s). Used for "Undo" which auto-applies Ctrl/Cmd. */
  forceMods?: ModKey[]
}

// Detect macOS so the Undo key sends Cmd+Z to mac hosts and Ctrl+Z elsewhere.
// `navigator.platform` is deprecated but still authoritative for OS detection;
// `navigator.userAgentData.platform` is gated behind permissions and not on
// every browser. The remote OS is what matters, but the client can't know it
// without an extra round-trip — assume the user runs the same OS family they
// remote into.
function isMacClient(): boolean {
  if (typeof navigator === 'undefined') return false
  const ua = (navigator.userAgent || '').toLowerCase()
  return /mac|iphone|ipad|ipod/.test(ua)
}

const UNDO_KEY: KeyDef = isMacClient()
  ? { label: 'Undo', code: 'KeyZ', forceMods: ['meta'] }
  : { label: 'Undo', code: 'KeyZ', forceMods: ['ctrl'] }

const KEYS: KeyDef[] = [
  { label: 'Esc', code: 'Escape' },
  UNDO_KEY,
  { label: 'Tab', code: 'Tab' },
  { label: 'Ctrl', code: '', mod: 'ctrl' },
  { label: 'Alt', code: '', mod: 'alt' },
  { label: 'Shift', code: '', mod: 'shift' },
  { label: '⌘', code: '', mod: 'meta' },
  { label: '⌫', code: 'Backspace' }, // backspace replaces ambiguous ✓
]

export default function DesktopToolbar({
  quality,
  onQualityChange,
  inputMode,
  onInputModeToggle,
  gestureMode = 'pointer',
  onGestureModeToggle,
  onKeyEvent,
  onShowGestureHelp,
  onShowOnscreenKeyboard,
  hidpi,
  smoothScaling,
  onSettingsChange,
  pipeline,
  onExit,
  textBatchOpen = false,
  onToggleTextBatch,
  audioGated = false,
  audioActive = false,
  onToggleAudio,
  pipelinePref,
  onPipelinePrefChange,
  showStats,
  onShowStatsChange,
}: Props) {
  const [activeModifiers, setActiveModifiers] = useState<Set<ModKey>>(new Set())
  const [settingsOpen, setSettingsOpen] = useState(false)
  // Wrap the gear button + popover so a single click-outside listener can
  // close the popover without each child needing its own ref.
  const settingsContainerRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!settingsOpen) return
    const onDown = (e: MouseEvent | TouchEvent) => {
      const target = e.target as Node | null
      if (target && !settingsContainerRef.current?.contains(target)) {
        setSettingsOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('touchstart', onDown)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('touchstart', onDown)
    }
  }, [settingsOpen])

  function toggleMod(mod: ModKey) {
    setActiveModifiers((prev) => {
      const next = new Set(prev)
      if (next.has(mod)) next.delete(mod)
      else next.add(mod)
      return next
    })
  }

  function dispatchKey(key: KeyDef) {
    if (key.mod) {
      toggleMod(key.mod)
      return
    }

    const forced = key.forceMods ?? []
    const mods = activeModifiers

    const ev: DesktopInputEvent = {
      t: 'key',
      code: key.code,
      action: 'down',
      ctrl: forced.includes('ctrl') || mods.has('ctrl'),
      alt: forced.includes('alt') || mods.has('alt'),
      shift: forced.includes('shift') || mods.has('shift'),
      meta: forced.includes('meta') || mods.has('meta'),
    }

    onKeyEvent(ev)
    onKeyEvent({ ...ev, action: 'up' })

    // Clear sticky modifiers after consumption
    if (mods.size > 0 || forced.length > 0) {
      setActiveModifiers(new Set())
    }
  }

  function keyBtnClass(key: KeyDef) {
    if (key.mod && activeModifiers.has(key.mod)) {
      return 'key-chip key-chip-once w-full !min-h-[2.25rem]'
    }
    return 'key-chip w-full !min-h-[2.25rem]'
  }

  return (
    <div className="flex flex-col gap-2 p-2 bg-surface border-t border-border lg:border-t-0 lg:border-l lg:h-full lg:w-60 lg:p-3">
      {/* Tools.
          Row 1 (full width): [Touch/Trackpad mode segmented control]
              — primary input mode deserves a clean, unambiguous label.
              Used to share row 2 with icon buttons + JPEG chip but the 240 px
              lg sidebar squeezed it to "T...". Own row solves it.
          Row 2: [Aa] [▢] [?] [Pipeline ▾] */}
      <div className="grid grid-cols-2 gap-1 p-0.5 rounded-md bg-surface-alt border border-border">
        <button
          onClick={inputMode === 'touch' ? undefined : onInputModeToggle}
          aria-pressed={inputMode === 'touch'}
          title="Touch (Direct): tap = click. Long press = right-click. Pinch to zoom; 2-finger swipe scrolls."
          className={[
            'h-8 text-xs font-medium rounded transition-colors',
            inputMode === 'touch'
              ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] shadow-[inset_0_0_0_1px_hsl(var(--accent-primary)/0.4)]'
              : 'text-text-muted hover:text-text-primary hover:bg-surface-hover',
          ].join(' ')}
        >
          Touch
        </button>
        <button
          onClick={inputMode === 'trackpad' ? undefined : onInputModeToggle}
          aria-pressed={inputMode === 'trackpad'}
          title="Trackpad: relative cursor. Tap = click. Long press = right-click. 2-finger swipe scrolls."
          className={[
            'h-8 text-xs font-medium rounded transition-colors',
            inputMode === 'trackpad'
              ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] shadow-[inset_0_0_0_1px_hsl(var(--accent-primary)/0.4)]'
              : 'text-text-muted hover:text-text-primary hover:bg-surface-hover',
          ].join(' ')}
        >
          Trackpad
        </button>
      </div>

      <div className="flex items-center gap-1.5 min-w-0">
        {onToggleTextBatch && (
          <button
            onClick={onToggleTextBatch}
            title="Text input — type long strings to the remote machine"
            aria-label="Toggle text input sheet"
            aria-pressed={textBatchOpen}
            className={[
              'shrink-0 inline-flex items-center justify-center w-8 h-8 border rounded-md transition-colors text-[13px] font-semibold',
              textBatchOpen
                ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]'
                : 'bg-surface-alt text-text-muted border-border hover:text-text-primary hover:bg-surface-hover',
            ].join(' ')}
          >
            Aa
          </button>
        )}

        {onGestureModeToggle && (
          <button
            onClick={onGestureModeToggle}
            title={
              gestureMode === 'rect'
                ? 'Rectangle-select mode is ON — drag to draw a marquee selection. Tap to switch back to pointer.'
                : 'Rectangle-select mode — drag to sweep a selection rectangle on the remote.'
            }
            aria-label="Toggle rectangle select"
            aria-pressed={gestureMode === 'rect'}
            className={[
              'shrink-0 inline-flex items-center justify-center w-8 h-8 border rounded-md transition-colors',
              gestureMode === 'rect'
                ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]'
                : 'bg-surface-alt text-text-muted border-border hover:text-text-primary hover:bg-surface-hover',
            ].join(' ')}
          >
            <svg
              viewBox="0 0 16 16"
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeDasharray="2 2"
              aria-hidden="true"
            >
              <rect x="2" y="2" width="12" height="12" rx="1" />
            </svg>
          </button>
        )}

        <button
          onClick={onShowGestureHelp}
          title="Gesture help"
          aria-label="Gesture help"
          className="shrink-0 inline-flex items-center justify-center w-8 h-8 border border-border rounded-md bg-surface-alt text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
        >
          <svg
            viewBox="0 0 16 16"
            width="14"
            height="14"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <circle cx="8" cy="8" r="6.5" />
            <path d="M6.2 6.2c0-1 .8-1.7 1.8-1.7s1.8.6 1.8 1.7c0 1.6-1.8 1.4-1.8 2.8" />
            <circle cx="8" cy="11" r="0.5" fill="currentColor" stroke="none" />
          </svg>
        </button>

        {/* Display popover trigger — pipeline label is the visual state.
            ml-auto pushes it to the right edge so row 2 reads as
            [tool icons …] [pipeline]. */}
        <div className="relative shrink-0 ml-auto" ref={settingsContainerRef}>
          <button
            type="button"
            onClick={() => setSettingsOpen((v) => !v)}
            aria-expanded={settingsOpen}
            aria-haspopup="dialog"
            title={
              pipeline === 'h264'
                ? 'Display — H.264 (WebRTC video track). Click to change quality and scaling.'
                : 'Display — JPEG (tile frames over DataChannel). Click to change quality and scaling.'
            }
            className={`inline-flex items-center gap-1 h-8 px-2 border rounded-md transition-colors text-[10px] font-semibold ${
              settingsOpen
                ? 'bg-accent/15 text-accent border-accent/40'
                : pipeline === 'h264'
                  ? 'bg-accent/10 text-accent border-accent/30 hover:bg-accent/20'
                  : 'bg-surface-alt text-text-muted border-border hover:text-text-primary hover:bg-surface-hover'
            }`}
          >
            {pipeline === 'h264' ? 'H.264' : 'JPEG'}
            <svg
              viewBox="0 0 16 16"
              width="10"
              height="10"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
              className={`transition-transform ${settingsOpen ? 'rotate-180' : ''}`}
              aria-hidden="true"
            >
              <path d="M4 6l4 4 4-4" />
            </svg>
          </button>
          {settingsOpen && (
            <DesktopSettingsPopover
              hidpi={hidpi}
              smoothScaling={smoothScaling}
              onChange={onSettingsChange}
              quality={quality}
              onQualityChange={onQualityChange}
              pipeline={pipeline}
              pipelinePref={pipelinePref}
              onPipelinePrefChange={onPipelinePrefChange}
              showStats={showStats}
              onShowStatsChange={onShowStatsChange}
            />
          )}
        </div>
      </div>

      {/* Modifier keys — 4×2 grid so labels never truncate (was 8×1 flex,
          which squeezed "Undo" → "Und" and "Shift" → "Shif" in the 240 px
          lg sidebar). Same key set, same dispatch logic. */}
      <div className="grid grid-cols-4 gap-1">
        {KEYS.map((key) => (
          <button
            key={key.label}
            onClick={() => dispatchKey(key)}
            className={keyBtnClass(key)}
            title={key.label}
          >
            {key.label}
          </button>
        ))}
      </div>

      {/* Phase-02a audio toggle — visible only when the gate passed at
          session-start (operator setting + probe + H.264 path).
          On → Off: instant client-driven mute via `audioToggle` over the WS;
          server tears the audio pipeline down with `UserToggleOff`, video
          continues uninterrupted.
          Off → On: triggers a session reconnect (same UX as Reload), since
          the SCK audio capture + Opus transceiver are bound at session start
          and webrtc-rs has no clean mid-session re-add path. ~1–2 s blip. */}
      {audioGated && (
        <div className="hidden lg:flex items-center justify-between gap-2 pt-1">
          <span className="text-[10px] uppercase tracking-[0.12em] text-text-muted">
            Audio
          </span>
          <button
            type="button"
            onClick={() => onToggleAudio?.()}
            title={
              audioActive
                ? 'Mute audio for this session'
                : 'Re-enable audio (triggers a session reconnect)'
            }
            className={
              'inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] border transition-colors ' +
              (audioActive
                ? 'border-accent/40 bg-accent/10 text-accent hover:bg-accent/20'
                : 'border-border bg-surface-alt text-text-secondary hover:bg-surface-hover hover:text-text-primary')
            }
          >
            <span
              className={
                'inline-block w-1.5 h-1.5 rounded-full ' +
                (audioActive ? 'bg-accent' : 'bg-text-muted')
              }
              aria-hidden="true"
            />
            {audioActive ? 'On' : 'Off'}
          </button>
        </div>
      )}

      {/* Transport indicator — lg only. Labelled so it doesn't read as an
          orphan pill; mobile sees the same info via DesktopTopStrip. */}
      <div className="hidden lg:flex items-center justify-end gap-2 pt-1">
        <span className="text-[10px] uppercase tracking-[0.12em] text-text-muted">
          Transport
        </span>
        <TransportPill compact />
      </div>

      {/* On-screen keyboard launcher — full key sheet (modifiers + arrows + Fn). */}
      {onShowOnscreenKeyboard && (
        <button
          onClick={onShowOnscreenKeyboard}
          className="text-xs py-1.5 rounded-md border border-border bg-surface-alt text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
        >
          ⌨ More keys
        </button>
      )}

      {/* Desktop-only Exit. The mobile path goes through DesktopTopStrip;
          rendering both lets the desktop sidebar match the same affordance. */}
      {onExit && (
        <button
          onClick={onExit}
          className="hidden lg:block text-xs py-1.5 rounded-md border border-danger/30 bg-danger/10 text-danger hover:bg-danger/20 transition-colors"
        >
          Exit remote desktop
        </button>
      )}
    </div>
  )
}
