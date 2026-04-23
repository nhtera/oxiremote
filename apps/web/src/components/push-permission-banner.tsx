import { useEffect, useState } from 'react'
import {
  currentSubscriptionState,
  detectPushSupport,
  subscribePush,
  unsubscribePush,
} from '../lib/push-client'

type SubState = 'subscribed' | 'unsubscribed' | 'denied' | 'unsupported' | 'loading'

const DISMISS_KEY = 'oxi:push-banner-dismissed'

export default function PushPermissionBanner() {
  const [state, setState] = useState<SubState>('loading')
  const [dismissed, setDismissed] = useState<boolean>(() => localStorage.getItem(DISMISS_KEY) === '1')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    currentSubscriptionState().then(setState)
  }, [])

  const support = detectPushSupport()
  if (support === 'requires-pwa-install') return null // InstallPwaBanner handles this
  if (state === 'loading' || state === 'unsupported' || state === 'subscribed') return null
  if (dismissed) return null

  async function onEnable() {
    setBusy(true)
    setErr(null)
    try {
      await subscribePush()
      setState('subscribed')
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
      const updated = await currentSubscriptionState()
      setState(updated)
    } finally {
      setBusy(false)
    }
  }

  function onDismiss() {
    localStorage.setItem(DISMISS_KEY, '1')
    setDismissed(true)
  }

  if (state === 'denied') {
    return (
      <div className="mx-3 my-2 rounded-lg border border-warning/30 bg-warning/5 px-3 py-2 text-xs">
        <div className="flex items-start gap-2">
          <div className="flex-1">
            <div className="text-warning font-medium mb-0.5">Notifications blocked</div>
            <div className="text-text-muted">
              Enable notifications in your browser settings to get pushes when builds finish or Claude Code pings.
            </div>
          </div>
          <button onClick={onDismiss} className="text-text-muted hover:text-text-primary px-1">✕</button>
        </div>
      </div>
    )
  }

  return (
    <div className="mx-3 my-2 rounded-lg border border-accent/30 bg-accent/5 px-3 py-2 text-xs">
      <div className="flex items-start gap-2">
        <div className="flex-1">
          <div className="text-text-primary font-medium mb-0.5">Get push notifications</div>
          <div className="text-text-muted mb-1.5">
            Builds, shell prompts, and deep links — tap to jump straight to the right terminal or file.
          </div>
          {err && <div className="text-danger mb-1.5">{err}</div>}
          <div className="flex gap-2">
            <button
              onClick={onEnable}
              disabled={busy}
              className="btn-primary text-xs py-1 px-2.5 disabled:opacity-50"
            >
              {busy ? 'Enabling…' : 'Enable'}
            </button>
            <button onClick={onDismiss} className="btn-secondary text-xs py-1 px-2.5">
              Not now
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

export function PushStatusRow() {
  const [state, setState] = useState<SubState>('loading')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    currentSubscriptionState().then(setState)
  }, [])

  async function onToggle() {
    setBusy(true)
    try {
      if (state === 'subscribed') {
        await unsubscribePush()
      } else {
        await subscribePush()
      }
    } catch (e) {
      console.error(e)
    } finally {
      setState(await currentSubscriptionState())
      setBusy(false)
    }
  }

  if (state === 'loading' || state === 'unsupported') return null

  const label =
    state === 'subscribed' ? 'Disable notifications' : state === 'denied' ? 'Notifications blocked' : 'Enable notifications'

  return (
    <button
      onClick={onToggle}
      disabled={busy || state === 'denied'}
      className="btn-secondary text-xs py-1 px-2.5 disabled:opacity-50"
    >
      {busy ? '…' : label}
    </button>
  )
}
