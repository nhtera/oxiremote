import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useHostStore } from '../state/host-store'
import { useWorkspaceStore } from '../state/workspace-store'
import FileAttachSheet from './file-attach-sheet'
import { PaperclipIcon } from './icons'
import { shellQuote } from '../lib/shell-quote'
import { ATTACHMENTS_DIR } from '../lib/ensure-attachments-dir'

type Props = {
  onSend: (bytes: string) => void
}

function useIsMobile() {
  const [mobile, setMobile] = useState(() =>
    typeof window !== 'undefined'
      ? window.matchMedia('(max-width: 768px)').matches
      : false
  )
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 768px)')
    const handler = (e: MediaQueryListEvent) => setMobile(e.matches)
    mq.addEventListener('change', handler)
    return () => mq.removeEventListener('change', handler)
  }, [])
  return mobile
}

export default function TerminalSendComposer({ onSend }: Props) {
  const isMobile = useIsMobile()
  const [text, setText] = useState('')
  const [showAttach, setShowAttach] = useState(false)
  const [needsVerbFlash, setNeedsVerbFlash] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const navigate = useNavigate()

  const currentHostId = useHostStore((s) => s.currentHostId)
  const activeMap = useWorkspaceStore((s) => s.active)
  const wsId = currentHostId ? activeMap[currentHostId]?.id : undefined
  const hasWs = wsId != null

  if (!isMobile) return null

  function send() {
    if (!text) return
    // Whitespace-prefix means insertPath put a path into an empty composer
    // and the user hit Send without typing a command verb. Sending the bare
    // path executes it as a command (`zsh: command not found: image.jpg`).
    // Refuse, park the caret at position 0, and flash a hint so the user
    // types a verb before the path.
    if (/^\s/.test(text)) {
      const el = inputRef.current
      el?.focus()
      el?.setSelectionRange(0, 0)
      setNeedsVerbFlash(true)
      window.setTimeout(() => setNeedsVerbFlash(false), 1400)
      return
    }
    onSend(text + '\r')
    setText('')
    inputRef.current?.focus()
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter') { e.preventDefault(); send() }
  }

  function insertPath(path: string) {
    const quoted = shellQuote(path)
    const el = inputRef.current
    if (!el) {
      setText((t) => (t ? `${t} ${quoted}` : quoted))
      return
    }
    const wasEmpty = text.length === 0
    const start = el.selectionStart ?? text.length
    const end = el.selectionEnd ?? text.length
    // When the composer was empty, prefix a space so the typed command verb
    // doesn't smash into the path (`catimage.jpg`). Caret then parks at 0 so
    // the user types the verb before the path — without this, an empty
    // composer + attach + Send sends the bare path, which the shell tries
    // to execute as a command (`zsh: command not found: image.jpg`).
    const insertion = wasEmpty ? ` ${quoted}` : quoted
    const next = text.slice(0, start) + insertion + text.slice(end)
    setText(next)
    requestAnimationFrame(() => {
      const pos = wasEmpty ? 0 : start + insertion.length
      el.focus()
      el.setSelectionRange(pos, pos)
    })
  }

  const canSend = text.trim().length > 0

  return (
    <>
      <div
        className="flex items-center gap-1.5 shrink-0 px-2 py-2 border-t border-border bg-surface-alt"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 0.5rem)' }}
      >
        <button
          type="button"
          onClick={() => {
            // Disabling the button when no active workspace was the original
            // gate, but a disabled <button> swallows the tap silently — users
            // saw "click does nothing" with no hint why. Route them to the
            // workspace picker instead so the affordance is recoverable.
            if (!hasWs) {
              if (currentHostId) navigate(`/h/${currentHostId}/workspaces`)
              return
            }
            setShowAttach(true)
          }}
          title={hasWs ? 'Attach file' : 'Pick a workspace to attach files'}
          aria-label={hasWs ? 'Attach file' : 'Pick a workspace to attach files'}
          className={`shrink-0 inline-flex items-center justify-center w-10 h-10 rounded-lg border bg-surface transition-colors hover:bg-surface-hover ${
            hasWs ? 'border-border text-accent' : 'border-border/60 text-text-muted'
          }`}
        >
          <PaperclipIcon size={18} />
        </button>
        <div className="flex-1 min-w-0 relative">
          <input
            ref={inputRef}
            type="text"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type command and send…"
            className={`w-full h-10 bg-surface border rounded-lg pl-3 pr-3 text-[15px] text-text-primary placeholder:text-text-muted outline-none focus:bg-surface/80 transition-colors ${
              needsVerbFlash
                ? 'border-warning ring-1 ring-warning/40'
                : 'border-border focus:border-accent/60'
            }`}
            style={{ fontFamily: 'var(--font-mono)' }}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
          />
          {needsVerbFlash && (
            <div
              role="status"
              className="absolute -top-7 left-0 right-0 text-[11px] text-warning bg-surface border border-warning/30 rounded px-2 py-1 shadow-sm pointer-events-none"
            >
              Type a command before the path (e.g. <span className="font-mono">cat</span>)
            </div>
          )}
        </div>
        <button
          onClick={send}
          disabled={!canSend}
          aria-label="Send"
          className={`shrink-0 inline-flex items-center justify-center w-10 h-10 rounded-lg text-white transition-all ${
            canSend
              ? 'bg-accent shadow-[0_4px_12px_-4px_rgba(255,122,64,0.6)] active:scale-95'
              : 'bg-surface border border-border text-text-muted'
          }`}
        >
          <svg viewBox="0 0 24 24" className="w-5 h-5" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M5 12h14" />
            <path d="M13 5l7 7-7 7" />
          </svg>
        </button>
      </div>

      {showAttach && wsId != null && (
        <FileAttachSheet
          wsId={wsId}
          dir={ATTACHMENTS_DIR}
          onPathInsert={insertPath}
          onClose={() => setShowAttach(false)}
        />
      )}
    </>
  )
}
