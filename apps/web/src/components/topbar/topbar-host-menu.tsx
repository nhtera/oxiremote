import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../../state/host-store'
import { ChevronDownIcon } from '../icons'
import { clearApiKey } from '../../lib/api-client'
import KeyboardShortcutOverlay from '../workspace/keyboard-shortcut-overlay'

// Top-bar dropdown surfacing the active host. Click → menu with the host
// label (green dot for "alive"), Switch host… (welcome screen lists saved
// pairings), and Logout. Mirrors the messaging-app convention of putting
// account/workspace switchers in the top-left.
const LONG_PRESS_MS = 600

export default function TopbarHostMenu() {
  const navigate = useNavigate()
  const { currentHostId, label } = useHostStore()
  const [open, setOpen] = useState(false)
  const [shortcutOpen, setShortcutOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  // Long-press state: timer ID while pointer is held down.
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const hostLabel = label ?? (currentHostId ? `${currentHostId.slice(0, 8)}…` : 'No host')

  useEffect(() => {
    if (!open) return
    function onClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false)
    }
    window.addEventListener('mousedown', onClick)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onClick)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  // Long-press on logo (600ms) opens the keyboard shortcut overlay on mobile.
  // `touch-action: manipulation` suppresses the iOS long-press context menu.
  function handleLogoPointerDown(e: React.PointerEvent) {
    // Only trigger for primary pointer (left button / touch point).
    if (e.button !== 0 && e.pointerType !== 'touch') return
    longPressTimer.current = setTimeout(() => {
      longPressTimer.current = null
      setOpen(false)
      setShortcutOpen(true)
    }, LONG_PRESS_MS)
  }

  function handleLogoPointerUp() {
    if (longPressTimer.current !== null) {
      clearTimeout(longPressTimer.current)
      longPressTimer.current = null
    }
  }

  // Suppress native context menu on logo (iOS long-press would otherwise
  // show the "Open Link / Copy Link / Share" sheet).
  function handleLogoContextMenu(e: React.MouseEvent) {
    e.preventDefault()
  }

  const handleLogout = async () => {
    await fetch('/api/auth/logout', { method: 'POST' })
    clearApiKey()
    setOpen(false)
    navigate('/login')
  }

  const handleSwitch = () => {
    setOpen(false)
    navigate('/welcome')
  }

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        onPointerDown={handleLogoPointerDown}
        onPointerUp={handleLogoPointerUp}
        onPointerLeave={handleLogoPointerUp}
        onContextMenu={handleLogoContextMenu}
        className="flex items-center gap-2 px-2 py-1.5 rounded-md hover:bg-surface-hover transition-colors"
        style={{ touchAction: 'manipulation' }}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span
          aria-hidden
          className="inline-flex h-6 w-6 items-center justify-center rounded-md bg-accent text-white"
        >
          <svg viewBox="0 0 24 24" className="w-3 h-3" fill="none" stroke="currentColor" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="4" width="18" height="13" rx="2" />
            <path d="M8 20h8" />
            <path d="M12 17v3" />
          </svg>
        </span>
        <span className="text-sm font-semibold text-text-primary tracking-tight">OxiRemote</span>
        <ChevronDownIcon size={14} />
        <span className="hidden sm:inline text-xs text-text-muted ml-1 max-w-[160px] truncate">
          {hostLabel}
        </span>
      </button>

      {open && (
        <div
          role="menu"
          className="absolute left-0 top-full mt-1 w-60 bg-surface border border-border rounded-md shadow-xl z-50 overflow-hidden"
        >
          <div className="px-3 py-2 border-b border-border flex items-center gap-2">
            <span className={`w-2 h-2 rounded-full shrink-0 ${currentHostId ? 'bg-success' : 'bg-text-muted'}`} />
            <span className="text-sm text-text-primary truncate">{hostLabel}</span>
          </div>
          <button
            type="button"
            role="menuitem"
            onClick={handleSwitch}
            className="w-full text-left px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            Switch host…
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={handleLogout}
            className="w-full text-left px-3 py-2 text-sm text-text-muted hover:text-danger hover:bg-surface-hover transition-colors border-t border-border"
          >
            Logout
          </button>
        </div>
      )}

      {/* Keyboard shortcut overlay — triggered by long-press on this logo. */}
      <KeyboardShortcutOverlay
        open={shortcutOpen}
        onClose={() => setShortcutOpen(false)}
      />
    </div>
  )
}
