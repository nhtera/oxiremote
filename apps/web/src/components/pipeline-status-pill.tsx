// Status pill for the active remote-desktop session — shows the chosen
// codec (H.264 / VP9 / AV1 / JPEG) with a tooltip explaining why.
//
// Lives in the desktop toolbar so users can self-diagnose unexpected
// fallback (e.g. iPad device without AV1 WebRTC, or a forced-jpeg env on
// the host). Tooltip carries the stable reason code from the agent so
// support docs can map ID → fix.

import StatusChip from './ui/status-chip'

export type PipelineMode = 'h264' | 'vp9' | 'av1' | 'jpeg'

interface Props {
  mode: PipelineMode
  /** Stable reason code from `pipeline_selection::Decision` on the agent. */
  reason: string
  /**
   * For video sessions: identifies whether the encoder backend is hardware-
   * accelerated. H.264 distinguishes VideoToolbox (Apple media stack) vs
   * OpenH264 (software); VP9 + AV1 are always software via libvpx/libaom
   * in phase-05/06. `undefined` while the encoder warms up (~33 ms).
   */
  hardwareAccel?: boolean
}

// Stable identifier → human-friendly tooltip. Keep the keys in lockstep
// with `agent/src/pipeline_selection.rs::Decision::reason`. Unknown reasons
// fall through to a generic label.
const REASON_TOOLTIPS: Record<string, string> = {
  'auto-h264': 'Auto-selected H.264 (client supports decode)',
  'auto-vp9': 'Auto-selected VP9 (client supports decode)',
  'auto-av1': 'Auto-selected AV1 (client supports decode)',
  'auto-jpeg-no-client': 'Auto fell back to JPEG (client cannot decode H.264)',
  'auto-jpeg-no-feature': 'Auto fell back to JPEG (server not built with H.264 support)',
  'forced-h264': 'H.264 forced by server operator (OXI_VIDEO_PIPELINE=h264)',
  'forced-vp9': 'VP9 forced by server operator (OXI_VIDEO_PIPELINE=vp9)',
  'forced-av1': 'AV1 forced by server operator (OXI_VIDEO_PIPELINE=av1)',
  'forced-jpeg': 'JPEG forced by server operator (OXI_VIDEO_PIPELINE=jpeg)',
  'forced-h264-no-client': 'Server forced H.264 but the client cannot decode it',
  'forced-vp9-no-client': 'Server forced VP9 but the client cannot decode it',
  'forced-av1-no-client': 'Server forced AV1 but the client cannot decode it (Safari has no AV1 WebRTC)',
}

function modeLabel(mode: PipelineMode, hardwareAccel: boolean | undefined): string {
  if (mode === 'jpeg') return 'JPEG'
  // VP9 + AV1 are software-only via libvpx/libaom in phase-05/06.
  if (mode === 'vp9') return hardwareAccel === true ? 'VP9 (HW)' : 'VP9 (SW)'
  if (mode === 'av1') return hardwareAccel === true ? 'AV1 (HW)' : 'AV1 (SW)'
  // H.264: backend name rather than HW/SW because VideoToolbox can silently
  // fall back to software on macOS VMs / CI runners.
  if (hardwareAccel === undefined) return 'H.264'
  return hardwareAccel ? 'H.264 (VT)' : 'H.264 (OpenH264)'
}

export default function PipelineStatusPill({ mode, reason, hardwareAccel }: Props) {
  // Variant choice: video sessions are the happy path → 'online' (green).
  // JPEG is a valid fallback but operators want a visual flag → 'info'.
  const variant = mode === 'jpeg' ? 'info' : 'online'
  const label = modeLabel(mode, hardwareAccel)
  const title = REASON_TOOLTIPS[reason] ?? `Pipeline: ${mode} (${reason})`

  return (
    <span title={title}>
      <StatusChip variant={variant} noDot={mode === 'jpeg'}>
        {label}
      </StatusChip>
    </span>
  )
}
