import { useEffect } from 'react'

// Measures the actual rendered chrome height (top bar + bottom tab bar)
// by reading the combined height of the first and last children of the
// CSS grid root, then exposes it as --app-chrome-height on <body>.
//
// FilesPage uses `calc(100dvh - var(--app-chrome-height, 8rem))` so the
// file panel fills exactly the visible area regardless of device / zoom.
//
// ResizeObserver re-runs on window resize and orientation change so the
// var stays accurate when the on-screen keyboard appears / disappears.
export function useAppChromeHeight() {
  useEffect(() => {
    const root = document.querySelector<HTMLElement>('[data-app-grid]')
    if (!root) return

    function measure() {
      if (!root) return
      const children = Array.from(root.children) as HTMLElement[]
      if (children.length < 2) return
      const first = children[0]
      const last = children[children.length - 1]
      // Skip the middle content row (grid row 2).
      const chrome = (first?.offsetHeight ?? 0) + (last?.offsetHeight ?? 0)
      document.body.style.setProperty('--app-chrome-height', `${chrome}px`)
    }

    measure()

    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(root)
    return () => ro.disconnect()
  }, [])
}
