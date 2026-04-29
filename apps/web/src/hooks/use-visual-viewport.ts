import { useEffect } from 'react'

// Subscribes to `window.visualViewport.resize` so callers can re-fit content
// when the iOS / Android soft keyboard opens or closes. The visual viewport
// shrinks when the keyboard appears; xterm + canvases need a fit() pass so
// the cursor isn't clipped behind the keyboard.
//
// The callback receives the current visual-viewport height. Calls are
// debounced via rAF so a stream of resize events collapses into one fit().
// Older browsers without `visualViewport` no-op gracefully — the layout
// still works, only adaptive refitting is missing.
export function useVisualViewport(cb: (height: number) => void): void {
  useEffect(() => {
    const vv = typeof window !== 'undefined' ? window.visualViewport : null
    if (!vv) return

    let raf = 0
    const fire = () => {
      if (raf) cancelAnimationFrame(raf)
      raf = requestAnimationFrame(() => cb(vv.height))
    }

    vv.addEventListener('resize', fire)
    vv.addEventListener('scroll', fire)
    // Initial sync after mount.
    fire()

    return () => {
      if (raf) cancelAnimationFrame(raf)
      vv.removeEventListener('resize', fire)
      vv.removeEventListener('scroll', fire)
    }
  }, [cb])
}
