import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../../state/host-store'
import { ChevronDownIcon } from '../icons'
import { clearApiKey } from '../../lib/api-client'
import { switchActiveHost } from '../../lib/host-switch-helpers'
import { listSavedHosts } from '../../lib/saved-hosts'
import KeyboardShortcutOverlay from '../workspace/keyboard-shortcut-overlay'

// Top-bar dropdown surfacing the active host. Click → menu with the host
// label (green dot for "alive"), saved-host list (in-place switch), Pair new
// host…, and Logout. Mirrors the messaging-app convention of putting
// account/workspace switchers in the top-left.
const LONG_PRESS_MS = 600

export default function TopbarHostMenu() {
  const navigate = useNavigate()
  const { currentHostId, label } = useHostStore()
  const [open, setOpen] = useState(false)
  const [shortcutOpen, setShortcutOpen] = useState(false)
  const [switchingId, setSwitchingId] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)
  // Long-press state: timer ID while pointer is held down.
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const hostLabel = label ?? (currentHostId ? `${currentHostId.slice(0, 8)}…` : 'No host')

  // Re-read on every render — listSavedHosts is a localStorage read, fine
  // for a dropdown that only mounts on click.
  const otherHosts = listSavedHosts().filter((h) => h.host_id !== currentHostId)

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

  function handleLogoContextMenu(e: React.MouseEvent) {
    e.preventDefault()
  }

  const handleLogout = async () => {
    await fetch('/api/auth/logout', { method: 'POST' })
    clearApiKey()
    setOpen(false)
    navigate('/login')
  }

  const handleSwitchTo = async (hostId: string) => {
    setSwitchingId(hostId)
    setOpen(false)
    await switchActiveHost(hostId, navigate)
    setSwitchingId(null)
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
        className="flex items-center gap-2 px-2 py-1.5 rounded-lg hover:bg-surface-hover transition-colors"
        style={{ touchAction: 'manipulation' }}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span
          aria-hidden
          className="relative inline-flex h-7 w-7 items-center justify-center rounded-lg bg-accent text-white shadow-[0_4px_12px_-4px_rgba(255,122,64,0.55)] ring-1 ring-white/10"
        >
          <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" fill="none" stroke="currentColor" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="4" width="18" height="13" rx="2" />
            <path d="M8 20h8" />
            <path d="M12 17v3" />
          </svg>
          {currentHostId && (
            <span aria-hidden className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-success ring-2 ring-surface-alt" />
          )}
        </span>
        <span className="text-sm font-semibold text-text-primary tracking-[-0.01em]">OxiRemote</span>
        <span className="hidden sm:flex items-center gap-1 ml-0.5 px-1.5 py-0.5 rounded-md bg-surface text-[10px] text-text-secondary border border-border max-w-[160px]">
          <span className="truncate">{hostLabel}</span>
          <ChevronDownIcon size={10} />
        </span>
        <span className="sm:hidden">
          <ChevronDownIcon size={14} />
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
          {otherHosts.length > 0 && (
            <div className="max-h-48 overflow-y-auto border-b border-border">
              {otherHosts.map((h) => (
                <button
                  key={h.host_id}
                  type="button"
                  role="menuitem"
                  disabled={switchingId !== null}
                  onClick={() => void handleSwitchTo(h.host_id)}
                  className="w-full text-left px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors flex items-center gap-2 disabled:opacity-60"
                >
                  {switchingId === h.host_id ? (
                    <span aria-hidden className="w-3 h-3 rounded-full border-2 border-current border-t-transparent animate-spin shrink-0" />
                  ) : (
                    <span className="w-1.5 h-1.5 rounded-full bg-text-muted shrink-0" />
                  )}
                  <span className="truncate">{h.label}</span>
                  {h.api_key_last4 && (
                    <span className="text-xs text-text-muted ml-auto shrink-0">····{h.api_key_last4}</span>
                  )}
                </button>
              ))}
            </div>
          )}
          <button
            type="button"
            role="menuitem"
            onClick={() => { setOpen(false); navigate('/welcome') }}
            className="w-full text-left px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary transition-colors"
          >
            Pair new host…
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
