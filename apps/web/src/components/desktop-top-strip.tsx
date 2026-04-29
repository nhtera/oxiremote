// Slim top toolbar shown only on small viewports (`lg:hidden`). Pairs with
// the bottom DesktopToolbar so a phone user has the full set of controls
// without leaving the session — back/exit, reload, screenshot, fullscreen.

interface Props {
  onExit: () => void
  onReload: () => void
  onFullscreen: () => void
  onScreenshot?: () => void
  inFullscreen: boolean
  /** Display scale of the canvas vs the remote's native resolution. 1.0 = 100%.
   *  When undefined we render `—%` as a skeleton so the slot doesn't pop. */
  zoom?: number
}

export default function DesktopTopStrip({
  onExit,
  onReload,
  onFullscreen,
  onScreenshot,
  inFullscreen,
  zoom,
}: Props) {
  const zoomLabel =
    typeof zoom === 'number' && zoom > 0 ? `${Math.round(zoom * 100)}%` : '—%'
  const zoomReady = typeof zoom === 'number' && zoom > 0
  return (
    <div className="lg:hidden fixed top-0 inset-x-0 z-30 pt-safe px-safe bg-surface/95 backdrop-blur border-b border-border">
      <div className="flex items-center gap-1 px-2 py-1">
      <IconButton onClick={onExit} aria-label="Exit remote desktop" title="Exit">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <line x1="19" y1="12" x2="5" y2="12" />
          <polyline points="12 19 5 12 12 5" />
        </svg>
      </IconButton>
      <span
        className={`px-1.5 text-[11px] tabular-nums transition-opacity ${
          zoomReady ? 'text-text-muted' : 'text-text-muted/50'
        }`}
        aria-label="Current zoom"
      >
        {zoomLabel}
      </span>
      <div className="flex-1" />
      <IconButton onClick={onReload} aria-label="Reload session" title="Reload">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <polyline points="1 4 1 10 7 10" />
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
        </svg>
      </IconButton>
      {onScreenshot && (
        <IconButton onClick={onScreenshot} aria-label="Take screenshot" title="Screenshot">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M4 7h3l2-2h6l2 2h3a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1z" />
            <circle cx="12" cy="13" r="3.5" />
          </svg>
        </IconButton>
      )}
      <IconButton
        onClick={onFullscreen}
        aria-label={inFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
        title={inFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
      >
        {inFullscreen ? (
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M9 3v6H3" /><path d="M15 3v6h6" /><path d="M3 15h6v6" /><path d="M21 15h-6v6" />
          </svg>
        ) : (
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3 9V3h6" /><path d="M21 9V3h-6" /><path d="M3 15v6h6" /><path d="M21 15v6h-6" />
          </svg>
        )}
      </IconButton>
      </div>
    </div>
  )
}

interface IconButtonProps {
  onClick: () => void
  children: React.ReactNode
  'aria-label': string
  title: string
}

function IconButton({ onClick, children, ...rest }: IconButtonProps) {
  return (
    <button
      onClick={onClick}
      className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors"
      {...rest}
    >
      <span className="block w-4 h-4">{children}</span>
    </button>
  )
}
