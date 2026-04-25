// Remote desktop toolbar — quality selector + 8-button mobile key row.
// Sticky modifiers (Ctrl/Alt/Shift/⌘): tap to toggle (orange highlight);
// the modifier is fused into the next key event then automatically cleared.
// On viewport ≥1024px this sits in a right sidebar; below that it's fixed bottom.

import { useEffect, useRef, useState } from 'react'
import type { QualityTier, DesktopInputEvent } from '../hooks/use-desktop-session'
import type { InputMode } from '../hooks/use-desktop-input'
import DesktopSettingsPopover from './desktop-settings-popover'

interface Props {
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  inputMode: InputMode
  onInputModeToggle: () => void
  onKeyEvent: (ev: DesktopInputEvent) => void
  onShowGestureHelp: () => void
  hidpi: boolean
  smoothScaling: boolean
  onSettingsChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
}

// Sticky modifier keys — toggled on tap, cleared after next key dispatch
type ModKey = 'ctrl' | 'alt' | 'shift' | 'meta'

interface KeyDef {
  label: string
  code: string
  mod?: ModKey
}

const KEYS: KeyDef[] = [
  { label: 'Esc', code: 'Escape' },
  { label: '⌃Z', code: 'KeyZ' },   // Undo — sends Ctrl+Z (ctrl fused in handler)
  { label: 'Tab', code: 'Tab' },
  { label: 'Ctrl', code: '', mod: 'ctrl' },
  { label: 'Alt', code: '', mod: 'alt' },
  { label: 'Shift', code: '', mod: 'shift' },
  { label: '⌘', code: '', mod: 'meta' },
  { label: '✓', code: 'Enter' },    // green send button
]

const QUALITY_OPTIONS: { value: QualityTier; label: string }[] = [
  { value: 'low', label: 'Low' },
  { value: 'med', label: 'Med' },
  { value: 'high', label: 'High' },
]

export default function DesktopToolbar({
  quality,
  onQualityChange,
  inputMode,
  onInputModeToggle,
  onKeyEvent,
  onShowGestureHelp,
  hidpi,
  smoothScaling,
  onSettingsChange,
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

    // Undo shortcut: ⌃Z label pre-wires Ctrl modifier
    const forceCtrl = key.label === '⌃Z'
    const mods = activeModifiers

    const ev: DesktopInputEvent = {
      t: 'key',
      code: key.code,
      action: 'down',
      ctrl: forceCtrl || mods.has('ctrl'),
      alt: mods.has('alt'),
      shift: mods.has('shift'),
      meta: mods.has('meta'),
    }

    onKeyEvent(ev)
    onKeyEvent({ ...ev, action: 'up' })

    // Clear sticky modifiers after consumption
    if (mods.size > 0 || forceCtrl) {
      setActiveModifiers(new Set())
    }
  }

  function keyBtnClass(key: KeyDef) {
    const base =
      'flex-1 min-w-0 py-2 text-xs font-medium rounded-md border border-border transition-colors select-none active:scale-95'
    if (key.mod && activeModifiers.has(key.mod)) {
      return `${base} bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]`
    }
    if (key.label === '✓') {
      return `${base} bg-green-600/20 text-green-400 border-green-600/40 hover:bg-green-600/30`
    }
    return `${base} bg-surface-alt text-text-secondary hover:bg-surface-hover hover:text-text-primary`
  }

  return (
    <div className="flex flex-col gap-2 p-2 bg-surface border-t border-border lg:border-t-0 lg:border-l lg:h-full lg:w-60 lg:p-3">
      {/* Quality + display settings row — both about render output */}
      <div className="flex items-center gap-2 min-w-0">
        <label className="text-xs text-text-muted shrink-0">Quality</label>
        <select
          value={quality}
          onChange={(e) => onQualityChange(e.target.value as QualityTier)}
          className="flex-1 min-w-0 text-xs bg-surface border border-border rounded-md px-2 py-1 text-text-primary outline-none focus:border-accent/50"
        >
          {QUALITY_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>

        <div className="relative shrink-0" ref={settingsContainerRef}>
          <button
            onClick={() => setSettingsOpen((v) => !v)}
            title="Display settings"
            aria-label="Display settings"
            aria-expanded={settingsOpen}
            className={`text-xs px-2 py-1 border border-border rounded-md transition-colors ${
              settingsOpen
                ? 'bg-surface-hover text-text-primary'
                : 'bg-surface-alt text-text-muted hover:text-text-primary hover:bg-surface-hover'
            }`}
          >
            ⚙
          </button>
          {settingsOpen && (
            <DesktopSettingsPopover
              hidpi={hidpi}
              smoothScaling={smoothScaling}
              onChange={onSettingsChange}
            />
          )}
        </div>
      </div>

      {/* Input mode + gesture help row */}
      <div className="flex items-center gap-2 min-w-0">
        <button
          onClick={onInputModeToggle}
          title={`Mode: ${inputMode}`}
          className="flex-1 min-w-0 text-xs px-2 py-1 border border-border rounded-md bg-surface-alt text-text-secondary hover:bg-surface-hover transition-colors"
        >
          {inputMode === 'touch' ? 'Touch' : 'Trackpad'}
        </button>

        <button
          onClick={onShowGestureHelp}
          title="Gesture help"
          className="shrink-0 text-xs px-2 py-1 border border-border rounded-md bg-surface-alt text-text-muted hover:text-text-primary hover:bg-surface-hover transition-colors"
          aria-label="Gesture help"
        >
          ?
        </button>
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
    </div>
  )
}
