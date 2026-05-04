import { useEffect } from 'react'

// Adds .visible to .reveal nodes once they enter the viewport.
// Single shared IntersectionObserver — cheap, runs once on mount.
export default function useScrollReveal() {
  useEffect(() => {
    const nodes = document.querySelectorAll('.reveal')
    if (nodes.length === 0) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add('visible')
            io.unobserve(entry.target)
          }
        }
      },
      { threshold: 0.12, rootMargin: '0px 0px -40px 0px' },
    )
    nodes.forEach((n) => io.observe(n))
    return () => io.disconnect()
  }, [])
}
