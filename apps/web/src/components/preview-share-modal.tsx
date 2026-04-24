import { useEffect, useState } from 'react'

interface ShareInfo {
  url: string
  expires_at: number
}

interface Props {
  info: ShareInfo
  onClose: () => void
}

export default function PreviewShareModal({ info, onClose }: Props) {
  const [copied, setCopied] = useState(false)
  const fullUrl = new URL(info.url, window.location.origin).toString()

  // auto-close the "copied" hint
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1500)
    return () => clearTimeout(t)
  }, [copied])

  // close on Escape
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(fullUrl)
      setCopied(true)
    } catch {
      // no-op; browser may block clipboard (insecure context / permission)
    }
  }

  const expiresInMin = Math.max(0, Math.round((info.expires_at * 1000 - Date.now()) / 60000))

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md bg-surface border border-border rounded-lg p-4 font-mono"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-sm font-semibold m-0 mb-2">Share preview</h3>
        <p className="text-text-muted text-xs mb-3">
          Signed URL — anyone with the link can reach this preview until it expires.
        </p>

        <label className="text-xs text-text-secondary block mb-1">URL</label>
        <textarea
          readOnly
          value={fullUrl}
          className="w-full h-20 px-2 py-1.5 text-xs bg-surface-alt border border-border rounded-md text-text-primary font-mono resize-none focus:outline-none focus:border-accent/50"
          onFocus={(e) => e.currentTarget.select()}
        />

        <div className="flex items-center justify-between mt-3">
          <span className="text-text-muted text-xs">
            Expires in ~{expiresInMin} min
          </span>
          <div className="flex gap-2">
            <button onClick={copy} className="btn-primary text-xs">
              {copied ? 'Copied!' : 'Copy'}
            </button>
            <button onClick={onClose} className="btn-secondary text-xs">
              Close
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
