// Bottom sheet for sending long text strings to the remote desktop.
// The user types (or pastes) text and hits Send; the hook dispatches a
// throttled sequence of key events (~30 ms/char) so remote input ordering
// is preserved even on slow tunnels.
// After Send the textarea is cleared and the sheet stays open — intent:
// let the user send a second value (e.g. username then password) without
// reopening the sheet.

import { useEffect, useRef } from 'react'
import Button from './ui/button'

interface Props {
  open: boolean
  onClose: () => void
  onSend: (text: string) => void
}

export default function DesktopTextBatchSheet({ open, onClose, onSend }: Props) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Auto-focus when sheet opens
  useEffect(() => {
    if (!open) return
    const id = window.setTimeout(() => {
      textareaRef.current?.focus()
    }, 50)
    return () => window.clearTimeout(id)
  }, [open])

  // Esc closes the sheet
  useEffect(() => {
    if (!open) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, onClose])

  function handleSend() {
    const text = textareaRef.current?.value ?? ''
    if (!text) return
    onSend(text)
    if (textareaRef.current) textareaRef.current.value = ''
    textareaRef.current?.focus()
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    // Shift+Enter or Ctrl+Enter sends without requiring the button tap
    if (e.key === 'Enter' && (e.shiftKey || e.ctrlKey)) {
      e.preventDefault()
      handleSend()
    }
  }

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-40 flex flex-col justify-end"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label="Text input sheet"
    >
      {/* Backdrop — semi-transparent so the canvas is still visible */}
      <div className="absolute inset-0 bg-black/40" aria-hidden="true" />

      {/* Sheet panel */}
      <div
        className="relative z-10 w-full bg-surface border-t border-border rounded-t-xl p-4"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 1rem)' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Drag handle + header */}
        <div className="mx-auto mb-3 h-1 w-10 rounded-full bg-border" />
        <div className="flex items-center justify-between mb-3">
          <span className="text-sm font-medium text-text-primary">Type to remote</span>
          <button
            onClick={onClose}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded"
            aria-label="Close text input sheet"
          >
            ✕
          </button>
        </div>

        {/* Textarea */}
        <textarea
          ref={textareaRef}
          rows={3}
          className={[
            'w-full resize-none rounded-md border border-border bg-surface-alt',
            'text-sm text-text-primary placeholder-text-muted',
            'px-3 py-2 outline-none focus:border-[hsl(var(--accent-primary)/0.6)]',
          ].join(' ')}
          placeholder="Paste or type text to send to the remote machine…"
          onKeyDown={handleKeyDown}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
        />

        <div className="flex items-center justify-between mt-2 gap-2">
          <span className="text-xs text-text-muted">Shift+Enter to send</span>
          <Button
            variant="accent-primary"
            size="sm"
            onClick={handleSend}
            aria-label="Send text to remote desktop"
          >
            Send
          </Button>
        </div>
      </div>
    </div>
  )
}
