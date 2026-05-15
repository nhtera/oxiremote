// Slim top toolbar shown only on small viewports (`lg:hidden`). After the
// CRD-aligned mobile redesign this is the *primary* control surface: the
// bottom DesktopToolbar is hidden on mobile, so everything operators reach
// for during a session lives here or in the composer / sheet.
//
// Layout (left → right):
//   [← back]  [Mode pill]  [⌨ keyboard sheet]  [zoom %]  [transport]  ...  [⋯]
//
// The overflow menu owns the long tail: reload, screenshot, fullscreen, fit
// zoom, marquee toggle, multiline sheet, gesture help, display settings.

import { useState } from 'react'
import type { QualityTier } from '../hooks/use-desktop-session'
import type { GestureMode, InputMode } from '../hooks/use-desktop-input'
import TransportPill from './transport-pill'
import DesktopOverflowMenu from './desktop-overflow-menu'

interface Props {
  onExit: () => void
  onReload: () => void
  onFullscreen: () => void
  onScreenshot?: () => void
  inFullscreen: boolean
  /** Display scale of the canvas vs the remote's native resolution. 1.0 = 100%.
   *  When undefined we render `—%` as a skeleton so the slot doesn't pop. */
  zoom?: number
  // CRD-aligned controls promoted from the (now lg-only) DesktopToolbar.
  inputMode: InputMode
  onInputModeToggle: () => void
  onShowKeyboard: () => void
  onShowTextBatch: () => void
  onResetZoom: () => void
  onShowGestureHelp: () => void
  gestureMode: GestureMode
  onGestureModeToggle: () => void
  hidpi: boolean
  smoothScaling: boolean
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  onSettingsChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
  pipeline: 'h264' | 'vp9' | 'av1' | 'jpeg'
  /** Phase-02a: whether the audio gate passed at session-start (operator
   *  setting + probe + video path). Drives chip visibility — hidden entirely
   *  when no audio path exists for this session. */
  audioGated?: boolean
  /** True when the audio pipeline is alive on the wire right now. */
  audioActive?: boolean
  /** Tap-to-toggle. On→Off mutes via WS (instant); Off→On reconnects. */
  onToggleAudio?: () => void
  /** User-driven pipeline preference — forwarded to the overflow menu picker. */
  pipelinePref?: 'auto' | 'av1' | 'vp9' | 'h264' | 'jpeg'
  onPipelinePrefChange?: (
    p: 'auto' | 'av1' | 'vp9' | 'h264' | 'jpeg',
  ) => void
  /** Phase-03 step 7: persisted "Show stream stats" toggle, forwarded to
   *  the overflow menu's Display section. */
  showStats?: boolean
  onShowStatsChange?: (v: boolean) => void
}

export default function DesktopTopStrip({
  onExit,
  onReload,
  onFullscreen,
  onScreenshot,
  inFullscreen,
  zoom,
  inputMode,
  onInputModeToggle,
  onShowKeyboard,
  onShowTextBatch,
  onResetZoom,
  onShowGestureHelp,
  gestureMode,
  onGestureModeToggle,
  hidpi,
  smoothScaling,
  quality,
  onQualityChange,
  onSettingsChange,
  pipeline,
  audioGated = false,
  audioActive = false,
  onToggleAudio,
  pipelinePref,
  onPipelinePrefChange,
  showStats,
  onShowStatsChange,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false)
  const zoomLabel =
    typeof zoom === 'number' && zoom > 0 ? `${Math.round(zoom * 100)}%` : '—%'
  const zoomReady = typeof zoom === 'number' && zoom > 0
  return (
    <div className="lg:hidden fixed top-0 inset-x-0 z-30 pt-safe px-safe bg-surface/85 backdrop-blur-md border-b border-border/80">
      <div className="relative flex items-center gap-1 px-2 py-1.5">
        <IconButton onClick={onExit} aria-label="Exit remote desktop" title="Exit">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <line x1="19" y1="12" x2="5" y2="12" />
            <polyline points="12 19 5 12 12 5" />
          </svg>
        </IconButton>

        {/* Mode toggle — single tap swaps Touch ↔ Trackpad. The label makes
            the current mode unambiguous; the icon hints at the swap. */}
        <button
          onClick={onInputModeToggle}
          aria-pressed={inputMode === 'trackpad'}
          title={
            inputMode === 'touch'
              ? 'Touch mode — tap = click at finger. Switch to Trackpad.'
              : 'Trackpad mode — virtual cursor follows finger. Switch to Touch.'
          }
          className={[
            'inline-flex items-center gap-1 h-9 px-2 rounded-lg text-[12px] font-semibold border transition-colors',
            inputMode === 'trackpad'
              ? 'bg-[hsl(var(--accent-primary)/0.18)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]'
              : 'bg-surface-alt text-text-secondary border-border hover:text-text-primary',
          ].join(' ')}
        >
          {inputMode === 'touch' ? (
            <TouchIcon />
          ) : (
            <CursorIcon />
          )}
          <span>{inputMode === 'touch' ? 'Touch' : 'Trackpad'}</span>
        </button>

        <IconButton onClick={onShowKeyboard} aria-label="Show keyboard" title="Show keyboard sheet">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <rect x="2.5" y="6" width="19" height="12" rx="1.5" />
            <path d="M6 10h.01M9 10h.01M12 10h.01M15 10h.01M18 10h.01M7 14h10" />
          </svg>
        </IconButton>

        <span
          className={`px-2 py-0.5 text-[11px] tabular rounded-md bg-surface-alt border border-border transition-opacity ${
            zoomReady ? 'text-text-secondary' : 'text-text-muted/50'
          }`}
          aria-label="Current zoom"
        >
          {zoomLabel}
        </span>
        <TransportPill compact />

        {audioGated && (
          <button
            type="button"
            onClick={onToggleAudio}
            aria-pressed={audioActive}
            title={audioActive ? 'Mute audio for this session' : 'Enable audio output'}
            className={[
              'inline-flex items-center gap-1 h-6 px-2 rounded-full text-[11px] font-medium border transition-colors',
              audioActive
                ? 'border-[hsl(var(--accent-primary)/0.4)] bg-[hsl(var(--accent-primary)/0.18)] text-[hsl(var(--accent-primary))]'
                : 'border-border bg-surface-alt text-text-secondary hover:text-text-primary hover:bg-surface-hover',
            ].join(' ')}
          >
            <span
              className={
                'inline-block w-1.5 h-1.5 rounded-full ' +
                (audioActive ? 'bg-[hsl(var(--accent-primary))]' : 'bg-text-muted')
              }
              aria-hidden="true"
            />
            {audioActive ? 'On' : 'Off'}
          </button>
        )}

        <div className="flex-1" />

        <IconButton
          onClick={() => setMenuOpen((v) => !v)}
          aria-label="Open session menu"
          aria-expanded={menuOpen}
          title="More"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <circle cx="5" cy="12" r="1.6" />
            <circle cx="12" cy="12" r="1.6" />
            <circle cx="19" cy="12" r="1.6" />
          </svg>
        </IconButton>

        <DesktopOverflowMenu
          open={menuOpen}
          onClose={() => setMenuOpen(false)}
          onReload={onReload}
          onScreenshot={onScreenshot}
          onFullscreen={onFullscreen}
          inFullscreen={inFullscreen}
          onResetZoom={onResetZoom}
          onShowGestureHelp={onShowGestureHelp}
          gestureMode={gestureMode}
          onGestureModeToggle={onGestureModeToggle}
          onShowTextBatch={onShowTextBatch}
          hidpi={hidpi}
          smoothScaling={smoothScaling}
          quality={quality}
          onQualityChange={onQualityChange}
          onSettingsChange={onSettingsChange}
          pipeline={pipeline}
          pipelinePref={pipelinePref}
          onPipelinePrefChange={onPipelinePrefChange}
          showStats={showStats}
          onShowStatsChange={onShowStatsChange}
        />
      </div>
    </div>
  )
}

interface IconButtonProps {
  onClick: () => void
  children: React.ReactNode
  'aria-label': string
  title: string
  'aria-expanded'?: boolean
}

function IconButton({ onClick, children, ...rest }: IconButtonProps) {
  return (
    <button
      onClick={onClick}
      onMouseDown={(e) => e.preventDefault()}
      className="inline-flex items-center justify-center w-9 h-9 rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-hover active:scale-95 transition-all"
      {...rest}
    >
      <span className="block w-4 h-4">{children}</span>
    </button>
  )
}

function TouchIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M7 8.5V4a1.5 1.5 0 1 1 3 0v5" />
      <path d="M10 6.5a1.5 1.5 0 1 1 3 0V11a3 3 0 0 1-3 3H8.6a3 3 0 0 1-2.5-1.4l-2-3.2a1 1 0 0 1 1-1.5L7 8.5" />
    </svg>
  )
}

function CursorIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 2 L3 12 L6 9 L7.8 13 L9.4 12.4 L7.6 8.5 L12 8.5 Z" />
    </svg>
  )
}
