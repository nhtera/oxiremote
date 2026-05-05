import { useEffect, useRef, useState } from 'react'
import { useHostStore } from '../state/host-store'
import { useWorkspaceStore } from '../state/workspace-store'
import FileAttachSheet from './file-attach-sheet'
import { PaperclipIcon } from './icons'

// POSIX-style shell quoting. If the path has nothing dangerous, returns it
// bare; otherwise wraps in single quotes and escapes embedded single quotes
// via the standard `'\''` terminate-escape-restart trick. Backslashes (Windows
// paths) are preserved verbatim by single-quote rules.
function shellQuote(path: string): string {
  if (!/[\s'"\\$`(){}[\]<>|&;*?#~!]/.test(path)) return path
  return `'${path.replace(/'/g, `'\\''`)}'`
}

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
  const inputRef = useRef<HTMLInputElement>(null)

  const currentHostId = useHostStore((s) => s.currentHostId)
  const activeMap = useWorkspaceStore((s) => s.active)
  const wsId = currentHostId ? activeMap[currentHostId]?.id : undefined

  if (!isMobile) return null

  function send() {
    if (!text) return
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
    const start = el.selectionStart ?? text.length
    const end = el.selectionEnd ?? text.length
    const next = text.slice(0, start) + quoted + text.slice(end)
    setText(next)
    // restore caret after the inserted text
    requestAnimationFrame(() => {
      const pos = start + quoted.length
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
          onClick={() => setShowAttach(true)}
          disabled={wsId == null}
          title={wsId == null ? 'Open a workspace to attach files' : 'Attach file'}
          aria-label="Attach file"
          className="shrink-0 inline-flex items-center justify-center w-10 h-10 rounded-lg border border-border bg-surface text-accent hover:bg-surface-hover disabled:opacity-40 transition-colors"
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
            className="w-full h-10 bg-surface border border-border rounded-lg pl-3 pr-3 text-[15px] text-text-primary placeholder:text-text-muted outline-none focus:border-accent/60 focus:bg-surface/80 transition-colors"
            style={{ fontFamily: 'var(--font-mono)' }}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
          />
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
          onPathInsert={insertPath}
          onClose={() => setShowAttach(false)}
        />
      )}
    </>
  )
}
