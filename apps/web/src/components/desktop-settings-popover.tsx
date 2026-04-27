// Display hub popover — quality, pipeline indicator, and per-client tweaks.
// Quality + pipeline live here (instead of inline rows in DesktopToolbar) so
// the toolbar stays focused on input/keys and the operator has one place for
// "render output" decisions.
//
// HiDPI mode: encoder gets native physical pixels (sharper text, ~3× CPU on
// agent). Toggling triggers a brief session reconnect — encoder dims are
// fixed at session start, so the agent rebuilds the pipeline.
//
// Smooth scaling: client-side only — flips the canvas 2D context's
// `imageSmoothingEnabled`. Off keeps text crisp on upscale; on smooths
// downscale aliasing.

import type { QualityTier } from '../hooks/use-desktop-session'
import StatusChip from './ui/status-chip'

interface Props {
  hidpi: boolean
  smoothScaling: boolean
  onChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  pipeline: 'h264' | 'jpeg'
}

const QUALITY_OPTIONS: { value: QualityTier; label: string }[] = [
  { value: 'low', label: 'Smooth' },
  { value: 'med', label: 'Balanced' },
  { value: 'high', label: 'Crisp' },
]

const PIPELINE_TOOLTIP: Record<'h264' | 'jpeg', string> = {
  h264: 'H.264 video stream — smaller bandwidth, hardware-decoded when available.',
  jpeg: 'JPEG tile stream — broader compatibility, more bandwidth.',
}

export default function DesktopSettingsPopover({
  hidpi,
  smoothScaling,
  onChange,
  quality,
  onQualityChange,
  pipeline,
}: Props) {
  return (
    <div
      className={
        // Mobile (toolbar fixed at bottom of viewport): popover goes UP into
        //   the canvas area — plenty of room above the bottom bar.
        // Desktop (toolbar = right sidebar with overflow-hidden ancestor):
        //   popover goes DOWN into the sidebar's empty space — going up
        //   would escape the parent's bounds and get clipped.
        'absolute right-0 w-64 rounded-xl border border-border bg-surface p-3 shadow-lg z-30 ' +
        'bottom-full mb-2 lg:bottom-auto lg:top-full lg:mt-2 lg:mb-0'
      }
    >
      <div className="flex items-center justify-between mb-2">
        <div className="text-xs font-medium text-text-primary">Display</div>
        <StatusChip
          variant={pipeline === 'h264' ? 'info' : 'offline'}
          noDot
          title={PIPELINE_TOOLTIP[pipeline]}
        >
          {pipeline === 'h264' ? 'H.264' : 'JPEG'}
        </StatusChip>
      </div>

      <label className="flex items-center gap-2 py-1.5">
        <span className="text-xs text-text-muted shrink-0 w-16">Quality</span>
        <select
          value={quality}
          onChange={(e) => onQualityChange(e.target.value as QualityTier)}
          className="flex-1 min-w-0 text-xs bg-surface-alt border border-border rounded-md px-2 py-1 text-text-primary outline-none focus:border-accent/50"
        >
          {QUALITY_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <div className="border-t border-border/60 mt-2 pt-2">
        <label className="flex items-start gap-2 cursor-pointer py-1">
          <input
            type="checkbox"
            checked={hidpi}
            onChange={(e) => onChange({ hidpi: e.target.checked, smoothScaling })}
            className="mt-0.5 accent-accent"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-text-primary">High-DPI mode</div>
            <div className="text-[10px] text-text-muted leading-tight">
              Crisper text on retina hosts. Reconnects the session.
            </div>
          </div>
        </label>

        <label className="flex items-start gap-2 cursor-pointer py-1">
          <input
            type="checkbox"
            checked={smoothScaling}
            onChange={(e) => onChange({ hidpi, smoothScaling: e.target.checked })}
            className="mt-0.5 accent-accent"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-text-primary">Smooth scaling</div>
            <div className="text-[10px] text-text-muted leading-tight">
              Off keeps text sharp; on softens aliasing on downscale.
            </div>
          </div>
        </label>
      </div>
    </div>
  )
}
