// Mobile overflow menu (⋯) anchored to the top strip's right edge. Holds
// the secondary controls that don't earn a dedicated icon on a phone-width
// strip: reload session, screenshot, fullscreen, fit-zoom, marquee toggle,
// gesture help, and display settings (HiDPI / smooth scaling / quality).
//
// Click-outside / Escape close. Items render as flat rows (no inner
// borders) so the menu reads as one panel; toggles use a right-aligned
// state chip that mirrors the value.

import { useEffect, useRef, type ReactNode } from 'react'
import type { QualityTier } from '../hooks/use-desktop-session'
import type { GestureMode } from '../hooks/use-desktop-input'

interface Props {
  open: boolean
  onClose: () => void
  // Actions
  onReload: () => void
  onScreenshot?: () => void
  onFullscreen: () => void
  inFullscreen: boolean
  onResetZoom: () => void
  onShowGestureHelp: () => void
  // Toggles
  gestureMode: GestureMode
  onGestureModeToggle: () => void
  onShowTextBatch: () => void
  // Display settings (forwarded to a popover-style sub-section)
  hidpi: boolean
  smoothScaling: boolean
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  onSettingsChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
  pipeline: 'h264' | 'vp9' | 'av1' | 'jpeg'
  /** User-driven pipeline preference. `auto` defers to agent default. */
  pipelinePref?: 'auto' | 'av1' | 'vp9' | 'h264' | 'jpeg'
  onPipelinePrefChange?: (
    p: 'auto' | 'av1' | 'vp9' | 'h264' | 'jpeg',
  ) => void
  /** Phase-03 step 7: persisted "Show stream stats" toggle. */
  showStats?: boolean
  onShowStatsChange?: (v: boolean) => void
}

export default function DesktopOverflowMenu({
  open,
  onClose,
  onReload,
  onScreenshot,
  onFullscreen,
  inFullscreen,
  onResetZoom,
  onShowGestureHelp,
  gestureMode,
  onGestureModeToggle,
  onShowTextBatch,
  hidpi,
  smoothScaling,
  quality,
  onQualityChange,
  onSettingsChange,
  pipeline,
  pipelinePref = 'auto',
  onPipelinePrefChange,
  showStats = false,
  onShowStatsChange,
}: Props) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent | TouchEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('touchstart', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('touchstart', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open, onClose])

  if (!open) return null

  const close = (fn?: () => void) => () => {
    fn?.()
    onClose()
  }

  return (
    <div
      ref={ref}
      role="menu"
      aria-label="Desktop session menu"
      className="absolute top-full right-2 mt-1 w-64 rounded-lg border border-border bg-surface shadow-xl z-40 overflow-hidden"
    >
      <Section>
        <Item icon={<ReloadIcon />} label="Reload session" onClick={close(onReload)} />
        {onScreenshot && (
          <Item
            icon={<CameraIcon />}
            label="Take screenshot"
            onClick={close(onScreenshot)}
          />
        )}
        <Item
          icon={inFullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />}
          label={inFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
          onClick={close(onFullscreen)}
        />
        <Item icon={<FitIcon />} label="Fit to screen (reset zoom)" onClick={close(onResetZoom)} />
      </Section>

      <Divider />

      <Section>
        <Item
          icon={<MarqueeIcon />}
          label="Rectangle select"
          trailing={<ToggleChip on={gestureMode === 'rect'} />}
          onClick={close(onGestureModeToggle)}
        />
        <Item
          icon={<TextIcon />}
          label="Multiline / paste sheet"
          onClick={close(onShowTextBatch)}
        />
        <Item icon={<HelpIcon />} label="Gesture help" onClick={close(onShowGestureHelp)} />
      </Section>

      <Divider />

      <Section title="Display">
        <Item
          icon={<HiDpiIcon />}
          label="High-DPI"
          trailing={<ToggleChip on={hidpi} />}
          onClick={close(() => onSettingsChange({ hidpi: !hidpi, smoothScaling }))}
        />
        <Item
          icon={<SmoothIcon />}
          label="Smooth scaling"
          trailing={<ToggleChip on={smoothScaling} />}
          onClick={close(() => onSettingsChange({ hidpi, smoothScaling: !smoothScaling }))}
        />
        {onShowStatsChange && (
          <Item
            icon={<StatsIcon />}
            label="Stream stats"
            trailing={<ToggleChip on={showStats} />}
            onClick={close(() => onShowStatsChange(!showStats))}
          />
        )}
        <div className="px-3 py-2 flex items-center gap-2">
          <span className="text-[11px] uppercase tracking-widest text-text-muted">Quality</span>
          <div className="flex-1 grid grid-cols-3 gap-1">
            {(['low', 'med', 'high'] as QualityTier[]).map((t) => {
              const active = quality === t
              return (
                <button
                  key={t}
                  onClick={() => onQualityChange(t)}
                  className={[
                    'h-7 text-[11px] font-medium rounded border transition-colors',
                    active
                      ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))] border-[hsl(var(--accent-primary)/0.4)]'
                      : 'bg-surface-alt text-text-muted border-border hover:text-text-primary',
                  ].join(' ')}
                >
                  {t === 'med' ? 'Med' : t.charAt(0).toUpperCase() + t.slice(1)}
                </button>
              )
            })}
          </div>
        </div>
        {/* Pipeline picker — segmented control, mirrors the lg-sidebar
            popover. `Auto` defers to the agent's chosen default; explicit
            picks ride on `?force_pipeline=` over the WS upgrade. The
            currently-active pipeline is shown in the bottom-row text. */}
        {onPipelinePrefChange && (
          <div className="px-3 py-2 flex flex-col gap-1">
            <span className="text-[11px] uppercase tracking-widest text-text-muted">
              Video codec
            </span>
            <select
              value={pipelinePref}
              onChange={(e) =>
                onPipelinePrefChange(
                  e.target.value as
                    | 'auto'
                    | 'av1'
                    | 'vp9'
                    | 'h264'
                    | 'jpeg',
                )
              }
              className="text-xs bg-surface-alt border border-border rounded-md px-2 py-1 text-text-primary outline-none focus:border-accent/50"
            >
              <option value="auto">Auto (recommended)</option>
              <option value="av1">AV1</option>
              <option value="vp9">VP9</option>
              <option value="h264">H.264</option>
              <option value="jpeg">JPEG (fallback)</option>
            </select>
          </div>
        )}
        <div className="px-3 pb-2 text-[10px] text-text-muted">
          Active: <span className="font-semibold text-text-secondary">{pipeline.toUpperCase()}</span>
        </div>
      </Section>
    </div>
  )
}

function Section({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <div className="py-1">
      {title && (
        <div className="px-3 pt-1 pb-0.5 text-[10px] uppercase tracking-[0.12em] text-text-muted">
          {title}
        </div>
      )}
      {children}
    </div>
  )
}

function Divider() {
  return <div className="h-px bg-border" />
}

interface ItemProps {
  icon: ReactNode
  label: string
  trailing?: ReactNode
  onClick: () => void
}

function Item({ icon, label, trailing, onClick }: ItemProps) {
  return (
    <button
      role="menuitem"
      onClick={onClick}
      onMouseDown={(e) => e.preventDefault()}
      className="w-full flex items-center gap-2.5 px-3 py-2 text-left text-sm text-text-primary hover:bg-surface-hover active:bg-surface-hover transition-colors"
    >
      <span className="w-4 h-4 text-text-secondary shrink-0">{icon}</span>
      <span className="flex-1">{label}</span>
      {trailing}
    </button>
  )
}

function ToggleChip({ on }: { on: boolean }) {
  return (
    <span
      className={[
        'inline-flex items-center justify-center text-[10px] font-semibold rounded px-1.5 py-0.5',
        on
          ? 'bg-[hsl(var(--accent-primary)/0.2)] text-[hsl(var(--accent-primary))]'
          : 'bg-surface-alt text-text-muted',
      ].join(' ')}
    >
      {on ? 'ON' : 'OFF'}
    </span>
  )
}

// ── Icons (16-grid, currentColor stroke) ────────────────────────────────────

function ReloadIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polyline points="1 3 1 7 5 7" />
      <path d="M2.6 10a6 6 0 1 0 1.4-6.2L1 7" />
    </svg>
  )
}
function CameraIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M2.5 5h2L6 3.5h4L11.5 5h2a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z" />
      <circle cx="8" cy="9" r="2.4" />
    </svg>
  )
}
function FullscreenIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
    </svg>
  )
}
function ExitFullscreenIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M6 2v4H2M10 2v4h4M6 14v-4H2M10 14v-4h4" />
    </svg>
  )
}
function FitIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
      <path d="M5 5l-2-2M11 5l2-2M5 11l-2 2M11 11l2 2" />
    </svg>
  )
}
function MarqueeIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeDasharray="2 2" aria-hidden="true">
      <rect x="2" y="2" width="12" height="12" rx="1" />
    </svg>
  )
}
function TextIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
      <path d="M3 4h10M3 8h10M3 12h6" />
    </svg>
  )
}
function HelpIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="8" cy="8" r="6.5" />
      <path d="M6.2 6.2c0-1 .8-1.7 1.8-1.7s1.8.6 1.8 1.7c0 1.6-1.8 1.4-1.8 2.8" />
      <circle cx="8" cy="11" r="0.5" fill="currentColor" stroke="none" />
    </svg>
  )
}
function HiDpiIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="2" y="3" width="12" height="9" rx="1" />
      <path d="M5 7v2M7.5 7v2M10 7v2M12.5 7v2" />
    </svg>
  )
}
function SmoothIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M2 11c2-4 4-4 6 0s4 4 6 0" />
    </svg>
  )
}

function StatsIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 13V8M7 13V4M11 13v-3M14 13H2" />
    </svg>
  )
}
