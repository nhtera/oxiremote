import { useState } from 'react'
import { detectPushSupport } from '../lib/push-client'

const DISMISS_KEY = 'oxi:install-pwa-banner-dismissed'

export default function InstallPwaBanner() {
  const [dismissed, setDismissed] = useState<boolean>(() => localStorage.getItem(DISMISS_KEY) === '1')

  // Only show on iOS Safari in a regular tab (A2HS is the only way to get Web Push there).
  if (detectPushSupport() !== 'requires-pwa-install') return null
  if (dismissed) return null

  function onDismiss() {
    localStorage.setItem(DISMISS_KEY, '1')
    setDismissed(true)
  }

  return (
    <div className="mx-3 my-2 rounded-lg border border-accent/30 bg-accent/5 px-3 py-2 text-xs">
      <div className="flex items-start gap-2">
        <div className="flex-1">
          <div className="text-text-primary font-medium mb-0.5">Install to get notifications</div>
          <div className="text-text-muted space-y-0.5">
            <div>iOS requires installing OxiRemote to get push notifications.</div>
            <div>
              Tap <span className="font-mono bg-surface px-1 rounded">Share</span> → <span className="font-mono bg-surface px-1 rounded">Add to Home Screen</span>, then open from your Home Screen.
            </div>
          </div>
        </div>
        <button onClick={onDismiss} className="text-text-muted hover:text-text-primary px-1">✕</button>
      </div>
    </div>
  )
}
