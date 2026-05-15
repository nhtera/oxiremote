// Display hub popover — quality, video codec, and per-client tweaks.
//
// Codec picker mirrors Chrome Remote Desktop's "Video codec" dropdown
// (AV1 → VP9 → H.264 → JPEG, plus Auto). Each codec row that the browser
// can't decode (Safari + AV1 most notably) is disabled with a tooltip
// explaining why.

import { useMemo } from 'react'

import {
  supportsAv1Video,
  supportsH264Video,
  supportsVp9Video,
} from '../hooks/codec-detect'
import type { QualityTier } from '../hooks/use-desktop-session'
import StatusChip from './ui/status-chip'

type PipelinePref = 'auto' | 'av1' | 'vp9' | 'h264' | 'jpeg'
type PipelineMode = 'h264' | 'vp9' | 'av1' | 'jpeg'

interface Props {
  hidpi: boolean
  smoothScaling: boolean
  onChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
  quality: QualityTier
  onQualityChange: (tier: QualityTier) => void
  /** Live signal of the codec the agent actually picked for this session. */
  pipeline: PipelineMode
  /** User-driven pipeline preference. `auto` defers to the agent's chosen
   *  default; explicit picks override per-session via `?force_pipeline=`. */
  pipelinePref?: PipelinePref
  onPipelinePrefChange?: (p: PipelinePref) => void
  /** Phase-03 step 7: persisted "Show stream stats" toggle. */
  showStats?: boolean
  onShowStatsChange?: (v: boolean) => void
}

const QUALITY_OPTIONS: { value: QualityTier; label: string }[] = [
  { value: 'low', label: 'Smooth' },
  { value: 'med', label: 'Balanced' },
  { value: 'high', label: 'Crisp' },
]

const PIPELINE_PILL_LABEL: Record<PipelineMode, string> = {
  h264: 'H.264',
  vp9: 'VP9',
  av1: 'AV1',
  jpeg: 'JPEG',
}

const PIPELINE_PILL_TOOLTIP: Record<PipelineMode, string> = {
  h264: 'H.264 video stream — hardware-decoded everywhere, ~12 Mbps at HIGH tier.',
  vp9: 'VP9 — screen-content-tuned libvpx, ~5 Mbps at HIGH tier (40-50 % of H.264).',
  av1: 'AV1 — libaom with AOM_CONTENT_SCREEN, ~3 Mbps at HIGH tier (best quality-per-bit).',
  jpeg: 'JPEG tile stream — broadest compatibility, more bandwidth than video codecs.',
}

interface CodecOption {
  value: PipelinePref
  label: string
  // `null` when supported. String describes why disabled.
  disabledReason: string | null
}

export default function DesktopSettingsPopover({
  hidpi,
  smoothScaling,
  onChange,
  quality,
  onQualityChange,
  pipeline,
  pipelinePref = 'auto',
  onPipelinePrefChange,
  showStats = false,
  onShowStatsChange,
}: Props) {
  // Probe browser capabilities once per render — these are synchronous and
  // cheap (RTCRtpReceiver.getCapabilities is cached by the browser).
  const codecOptions: CodecOption[] = useMemo(() => {
    const av1 = supportsAv1Video()
    const vp9 = supportsVp9Video()
    const h264 = supportsH264Video()
    return [
      { value: 'auto', label: 'Auto (recommended)', disabledReason: null },
      {
        value: 'av1',
        label: 'AV1',
        disabledReason: av1
          ? null
          : "This browser doesn't support AV1 over WebRTC. Safari has no AV1 plans as of 2026.",
      },
      {
        value: 'vp9',
        label: 'VP9',
        disabledReason: vp9 ? null : "This browser doesn't expose VP9 in WebRTC.",
      },
      {
        value: 'h264',
        label: 'H.264',
        disabledReason: h264 ? null : "This browser doesn't expose RTCPeerConnection.",
      },
      { value: 'jpeg', label: 'JPEG (fallback)', disabledReason: null },
    ]
  }, [])

  return (
    <div
      className={
        'absolute right-0 w-72 rounded-xl border border-border bg-surface p-3 shadow-lg z-30 ' +
        'bottom-full mb-2 lg:bottom-auto lg:top-full lg:mt-2 lg:mb-0'
      }
    >
      <div className="flex items-center justify-between mb-2">
        <div className="text-xs font-medium text-text-primary">Display</div>
        <StatusChip
          variant={pipeline === 'jpeg' ? 'offline' : 'info'}
          noDot
          title={PIPELINE_PILL_TOOLTIP[pipeline]}
        >
          {PIPELINE_PILL_LABEL[pipeline]}
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

      {/* Codec picker — mirrors Chrome Remote Desktop's "Video codec" dropdown.
          AV1 → VP9 → H.264 → JPEG. Unsupported rows are disabled with a
          tooltip explaining why. Mid-session change triggers a reconnect. */}
      {onPipelinePrefChange && (
        <label className="flex items-center gap-2 py-1.5">
          <span className="text-xs text-text-muted shrink-0 w-16">Codec</span>
          <select
            value={pipelinePref}
            onChange={(e) => onPipelinePrefChange(e.target.value as PipelinePref)}
            className="flex-1 min-w-0 text-xs bg-surface-alt border border-border rounded-md px-2 py-1 text-text-primary outline-none focus:border-accent/50"
          >
            {codecOptions.map((opt) => (
              <option
                key={opt.value}
                value={opt.value}
                disabled={opt.disabledReason !== null}
                title={opt.disabledReason ?? undefined}
              >
                {opt.label}
                {opt.disabledReason ? ' (unavailable)' : ''}
              </option>
            ))}
          </select>
        </label>
      )}

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
              Anti-alias the canvas — softer text, fewer jaggies on resize.
            </div>
          </div>
        </label>
        {onShowStatsChange && (
          <label className="flex items-start gap-2 cursor-pointer py-1">
            <input
              type="checkbox"
              checked={showStats}
              onChange={(e) => onShowStatsChange(e.target.checked)}
              className="mt-0.5 accent-accent"
            />
            <div className="flex-1 min-w-0">
              <div className="text-xs text-text-primary">Show stream stats</div>
              <div className="text-[10px] text-text-muted leading-tight">
                Overlay encode/decode timing + bitrate.
              </div>
            </div>
          </label>
        )}
      </div>
    </div>
  )
}
