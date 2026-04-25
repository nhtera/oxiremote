// Phase 04 quality controls — small popover with two boolean toggles.
//
// HiDPI mode: encoder gets native physical pixels (sharper text, ~3× CPU on
// agent). Toggling triggers a brief session reconnect — encoder dims are
// fixed at session start, so the agent rebuilds the pipeline.
//
// Smooth scaling: client-side only — flips the canvas 2D context's
// `imageSmoothingEnabled`. Off keeps text crisp on upscale; on smooths
// downscale aliasing.

interface Props {
  hidpi: boolean
  smoothScaling: boolean
  onChange: (next: { hidpi: boolean; smoothScaling: boolean }) => void
}

export default function DesktopSettingsPopover({ hidpi, smoothScaling, onChange }: Props) {
  return (
    <div
      className={
        // Mobile (toolbar fixed at bottom of viewport): popover goes UP into
        //   the canvas area — plenty of room above the bottom bar.
        // Desktop (toolbar = right sidebar with overflow-hidden ancestor):
        //   popover goes DOWN into the sidebar's empty space — going up
        //   would escape the parent's bounds and get clipped.
        'absolute right-0 w-64 rounded-md border border-border bg-surface p-3 shadow-lg z-30 ' +
        'bottom-full mb-2 lg:bottom-auto lg:top-full lg:mt-2 lg:mb-0'
      }
    >
      <div className="text-xs font-medium text-text-primary mb-2">Display</div>

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
  )
}
