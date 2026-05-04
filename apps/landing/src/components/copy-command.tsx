import { useState } from 'react'

interface CopyCommandProps {
  command: string
  prefix?: string
}

export default function CopyCommand({ command, prefix = '$' }: CopyCommandProps) {
  const [copied, setCopied] = useState(false)

  async function copy() {
    try {
      await navigator.clipboard.writeText(command)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      /* user denied clipboard — silently no-op */
    }
  }

  return (
    <button
      type="button"
      onClick={copy}
      aria-label="Copy install command"
      className="group flex items-center gap-3 w-full max-w-md rounded-xl bg-surface-alt/70 backdrop-blur-sm border border-border px-4 h-12 text-left font-mono text-[13.5px] hover:border-text-secondary transition-colors"
    >
      <span className="text-text-muted select-none">{prefix}</span>
      <span className="flex-1 truncate text-text-primary">{command}</span>
      <span
        className={`text-[11px] tracking-wide font-sans uppercase transition-colors ${
          copied ? 'text-success' : 'text-text-muted group-hover:text-text-secondary'
        }`}
      >
        {copied ? 'copied' : 'copy'}
      </span>
    </button>
  )
}
