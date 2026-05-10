// Status pill for the active remote-desktop session — shows whether the
// agent picked H.264 (HW/SW) or JPEG, and why.
//
// Lives in the desktop toolbar so users can self-diagnose unexpected JPEG
// sessions (e.g. iPad device that fell off the H.264 capability list, or
// a forced-jpeg env on the host). Tooltip surface carries the stable
// reason code from the agent so support docs can map ID → fix.
//
// Bundle: zero new deps, reuses the existing StatusChip — well under the
// 0.5 KB gz pill budget.

import StatusChip from './ui/status-chip'

export type PipelineMode = 'h264' | 'jpeg'

interface Props {
  mode: PipelineMode
  /** Stable reason code from `pipeline_selection::Decision` on the agent. */
  reason: string
  /**
   * For H.264 sessions only: identifies the encoder backend.
   * `true` = VideoToolbox (Apple media stack); `false` = OpenH264 (software).
   * The label says "VT" / "OpenH264" rather than "HW" / "SW" — VideoToolbox
   * is a framework and silently uses software on macOS VMs / CI runners /
   * Rosetta, so a "HW" badge would be a lie there. The framework name is
   * accurate without that risk. `undefined` while the encoder warms up
   * (~33 ms after stream-up) to avoid flashing the wrong badge.
   */
  hardwareAccel?: boolean
}

// Stable identifier → human-friendly tooltip. Keep the keys in lockstep
// with `agent/src/pipeline_selection.rs::Decision::reason`. Unknown reasons
// fall through to a generic label rather than a confusing crash — useful
// during a rolling deploy when the server may emit a newer reason code
// than the SPA knows about.
const REASON_TOOLTIPS: Record<string, string> = {
  'auto-h264': 'Auto-selected H.264 (client supports decode)',
  'auto-jpeg-no-client': 'Auto fell back to JPEG (client cannot decode H.264)',
  'auto-jpeg-no-feature': 'Auto fell back to JPEG (server not built with H.264 support)',
  'forced-h264': 'H.264 forced by server operator (OXI_VIDEO_PIPELINE=h264)',
  'forced-jpeg': 'JPEG forced by server operator (OXI_VIDEO_PIPELINE=jpeg)',
  'forced-h264-no-client': 'Server forced H.264 but the client cannot decode it',
}

function modeLabel(mode: PipelineMode, hardwareAccel: boolean | undefined): string {
  if (mode === 'jpeg') return 'JPEG'
  if (hardwareAccel === undefined) return 'H.264'
  // Backend names rather than HW/SW because VideoToolbox can silently fall
  // back to software on macOS VMs / CI runners. The label reflects which
  // *encoder* is running, which is always knowable. Tooltip carries the
  // decision reason if the user wants more context.
  return hardwareAccel ? 'H.264 (VT)' : 'H.264 (OpenH264)'
}

export default function PipelineStatusPill({ mode, reason, hardwareAccel }: Props) {
  // Variant choice: H.264 sessions are the happy path → 'online' (green dot).
  // JPEG sessions are *valid* — the fallback works — but operators want a
  // visual flag so they notice unexpected fallback rates. 'info' (accent
  // blue) signals "informational, not an error".
  const variant = mode === 'h264' ? 'online' : 'info'
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
