import { useEffect, useRef, useState } from 'react'
import { useHostStore } from '../state/host-store'
import { useWorkspaceStore } from '../state/workspace-store'
import FileAttachSheet from './file-attach-sheet'

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
    const quoted = `"${path}"`
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

  return (
    <>
      <div
        className="flex gap-2 shrink-0 px-2 py-1.5 border-t border-border bg-surface-alt"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 0.375rem)' }}
      >
        <button
          type="button"
          onClick={() => setShowAttach(true)}
          disabled={wsId == null}
          title={wsId == null ? 'Open a workspace to attach files' : 'Attach file'}
          aria-label="Attach file"
          className="btn-secondary text-base px-2 py-1.5 disabled:opacity-40"
        >
          📎
        </button>
        <input
          ref={inputRef}
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Type command…"
          className="flex-1 min-w-0 bg-surface border border-border rounded-md px-2 py-1.5 text-sm text-text-primary placeholder:text-text-muted outline-none focus:border-accent/50"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <button
          onClick={send}
          disabled={!text}
          className="btn-primary text-xs px-3 py-1.5 disabled:opacity-40"
        >
          Send
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
