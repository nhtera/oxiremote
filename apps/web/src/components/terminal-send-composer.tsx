import { useEffect, useRef, useState } from 'react'

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
  const inputRef = useRef<HTMLInputElement>(null)

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

  return (
    <div className="flex gap-2 shrink-0 px-2 py-1.5 border-t border-border bg-surface-alt">
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
  )
}
