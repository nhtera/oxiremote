// Slim top toolbar shown only on small viewports (`lg:hidden`). Pairs with
// the bottom DesktopToolbar so a phone user has the full set of controls
// without leaving the session — back/exit, reload, fullscreen — instead of
// browser-back which loses click-and-drag state.

interface Props {
  onExit: () => void
  onReload: () => void
  onFullscreen: () => void
  inFullscreen: boolean
}

export default function DesktopTopStrip({
  onExit,
  onReload,
  onFullscreen,
  inFullscreen,
}: Props) {
  return (
    <div className="lg:hidden fixed top-0 inset-x-0 z-30 flex items-center gap-1 px-2 py-1 bg-surface/95 backdrop-blur border-b border-border">
      <IconButton onClick={onExit} aria-label="Exit remote desktop" title="Exit">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <line x1="19" y1="12" x2="5" y2="12" />
          <polyline points="12 19 5 12 12 5" />
        </svg>
      </IconButton>
      <div className="flex-1" />
      <IconButton onClick={onReload} aria-label="Reload session" title="Reload">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <polyline points="1 4 1 10 7 10" />
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
        </svg>
      </IconButton>
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
