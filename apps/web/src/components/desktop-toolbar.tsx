// Remote desktop toolbar — quality selector + 8-button mobile key row.
// Sticky modifiers (Ctrl/Alt/Shift/⌘): tap to toggle (accent highlight);
// the modifier is fused into the next key event then automatically cleared.
// On viewport ≥1024px this sits in a right sidebar; below that it's fixed bottom.

import { useEffect, useRef, useState } from 'react'
import type { QualityTier, DesktopInputEvent } from '../hooks/use-desktop-session'
import type { InputMode, GestureMode } from '../hooks/use-desktop-input'
import DesktopSettingsPopover from './desktop-settings-popover'
import TransportPill from './transport-pill'
import { SettingsIcon } from './icons'

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
    const base =
      'flex-1 min-w-0 py-2 text-xs font-medium rounded-md border border-border transition-colors select-none active:scale-95'
    if (key.mod && activeModifiers.has(key.mod)) {
      return `${base} bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]`
    }
    return `${base} bg-surface-alt text-text-secondary hover:bg-surface-hover hover:text-text-primary`
  }

  return (
    <div className="flex flex-col gap-2 p-2 bg-surface border-t border-border lg:border-t-0 lg:border-l lg:h-full lg:w-60 lg:p-3">
      {/* Input mode + Aa text-batch + gesture help + display gear row.
          Layout: [Touch/Trackpad toggle] [Aa] [?] [⚙]
          Aa is orange when the text-batch sheet is open.
          ⚙ opens the Display popover (quality, pipeline, hidpi, scaling). */}
      <div className="flex items-center gap-2 min-w-0">
        <button
          onClick={onInputModeToggle}
          title={
            inputMode === 'touch'
              ? 'Touch (Direct): tap = click at finger position. Drag with one finger to move the pointer.'
              : 'Trackpad: relative cursor. Two-finger swipe scrolls; tap clicks at the cursor.'
          }
          className="flex-1 min-w-0 text-xs px-2 py-1 border border-border rounded-md bg-surface-alt text-text-secondary hover:bg-surface-hover transition-colors"
        >
          {inputMode === 'touch' ? 'Touch' : 'Trackpad'}
        </button>

        {onToggleTextBatch && (
          <button
            onClick={onToggleTextBatch}
            title="Text input — type long strings to the remote machine"
            aria-label="Toggle text input sheet"
            aria-pressed={textBatchOpen}
            className={[
              'shrink-0 text-xs px-2 py-1 border rounded-md transition-colors font-medium',
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
              'shrink-0 text-xs px-2 py-1 border rounded-md transition-colors font-medium',
              gestureMode === 'rect'
                ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]'
                : 'bg-surface-alt text-text-muted border-border hover:text-text-primary hover:bg-surface-hover',
            ].join(' ')}
          >
            ▢
          </button>
        )}

        <button
          onClick={onShowGestureHelp}
          title="Gesture help"
          aria-label="Gesture help"
          className="shrink-0 text-xs px-2 py-1 border border-border rounded-md bg-surface-alt text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
        >
          ?
        </button>

        <div className="relative shrink-0 flex items-center gap-1.5" ref={settingsContainerRef}>
          <button
            type="button"
            onClick={() => setSettingsOpen((v) => !v)}
            aria-label={`Active video pipeline: ${pipeline === 'h264' ? 'H.264' : 'JPEG'}. Click to open display settings.`}
            title={
              pipeline === 'h264'
                ? 'H.264 — WebRTC video track'
                : 'JPEG — tile frames over DataChannel'
            }
            className={`text-[10px] font-semibold px-1.5 py-0.5 rounded transition-colors ${
              pipeline === 'h264'
                ? 'bg-accent/15 text-accent border border-accent/30 hover:bg-accent/25'
                : 'bg-surface-alt text-text-muted border border-border hover:text-text-primary'
            }`}
          >
            {pipeline === 'h264' ? 'H.264' : 'JPEG'}
          </button>
          <button
            onClick={() => setSettingsOpen((v) => !v)}
            title="Display settings (quality, pipeline)"
            aria-label="Display settings"
            aria-expanded={settingsOpen}
            className={`inline-flex items-center justify-center w-8 h-7 border border-border rounded-md transition-colors ${
              settingsOpen
                ? 'bg-surface-hover text-text-primary'
                : 'bg-surface-alt text-text-muted hover:text-text-primary hover:bg-surface-hover'
            }`}
          >
            <SettingsIcon size={14} />
          </button>
          {settingsOpen && (
            <DesktopSettingsPopover
              hidpi={hidpi}
              smoothScaling={smoothScaling}
              onChange={onSettingsChange}
              quality={quality}
              onQualityChange={onQualityChange}
              pipeline={pipeline}
            />
          )}
        </div>
      </div>

      {/* 8-button key row */}
      <div className="flex gap-1">
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

      {/* Transport indicator — lg only; mobile sees this via DesktopTopStrip. */}
      <div className="hidden lg:flex items-center justify-end pt-1">
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
