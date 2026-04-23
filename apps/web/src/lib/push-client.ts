// Web Push client: registers the service worker, subscribes with VAPID, and
// posts the subscription (endpoint + keys) to the agent. Keeps "subscription
// status" derivable from Notification.permission + pushManager — no server
// round-trip needed to render a button state.

const SW_URL = '/service-worker.js'

export type PushSupport =
  | 'supported'
  | 'unsupported-browser'
  | 'requires-pwa-install' // iOS Safari in a regular tab

export function detectPushSupport(): PushSupport {
  if (typeof window === 'undefined') return 'unsupported-browser'
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) {
    return 'unsupported-browser'
  }
  // iOS Safari only exposes Notification + PushManager when installed (A2HS).
  const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent) && !(window as unknown as { MSStream?: unknown }).MSStream
  const standalone =
    (window.matchMedia && window.matchMedia('(display-mode: standalone)').matches) ||
    // Safari legacy
    (navigator as unknown as { standalone?: boolean }).standalone === true
  if (isIOS && !standalone) return 'requires-pwa-install'
  return 'supported'
}

export async function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
  if (!('serviceWorker' in navigator)) return null
  try {
    return await navigator.serviceWorker.register(SW_URL, { scope: '/' })
  } catch (e) {
    console.error('[push] SW register failed', e)
    return null
  }
}

function base64UrlToBuffer(b64: string): ArrayBuffer {
  const padding = '='.repeat((4 - (b64.length % 4)) % 4)
  const base64 = (b64 + padding).replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(base64)
  const buf = new ArrayBuffer(raw.length)
  const arr = new Uint8Array(buf)
  for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i)
  return buf
}

function arrayBufferToBase64Url(buf: ArrayBuffer | null): string {
  if (!buf) return ''
  const bytes = new Uint8Array(buf)
  let bin = ''
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i])
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

async function fetchVapidPublicKey(): Promise<string> {
  const res = await fetch('/api/push/vapid-public', { credentials: 'include' })
  if (!res.ok) throw new Error(`vapid-public: ${res.status}`)
  const data = (await res.json()) as { public_key: string }
  return data.public_key
}

export async function subscribePush(): Promise<PushSubscription> {
  const reg = await registerServiceWorker()
  if (!reg) throw new Error('service worker not registered')

  const permission = await Notification.requestPermission()
  if (permission !== 'granted') throw new Error(`permission ${permission}`)

  const vapidKey = await fetchVapidPublicKey()
  // Always subscribe — idempotent when the VAPID key matches an existing
  // subscription, and correctly replaces stale subs from a rotated key.
  // Reusing pushManager.getSubscription() here would silently break pushes
  // after a key rotation (FCM/Mozilla would 401 every send).
  const sub = await reg.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: base64UrlToBuffer(vapidKey),
  })

  const p256dh = arrayBufferToBase64Url(sub.getKey('p256dh'))
  const auth = arrayBufferToBase64Url(sub.getKey('auth'))

  const res = await fetch('/api/push/subscribe', {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ endpoint: sub.endpoint, p256dh, auth }),
  })
  if (!res.ok) throw new Error(`subscribe: ${res.status}`)

  return sub
}

export async function unsubscribePush(): Promise<void> {
  if (!('serviceWorker' in navigator)) return
  const reg = await navigator.serviceWorker.getRegistration()
  const sub = await reg?.pushManager.getSubscription()
  if (sub) await sub.unsubscribe()

  await fetch('/api/push/subscribe', {
    method: 'DELETE',
    credentials: 'include',
  })
}

export async function currentSubscriptionState(): Promise<'subscribed' | 'unsubscribed' | 'denied' | 'unsupported'> {
  const support = detectPushSupport()
  if (support !== 'supported') return 'unsupported'
  if (Notification.permission === 'denied') return 'denied'
  const reg = await navigator.serviceWorker.getRegistration()
  const sub = await reg?.pushManager.getSubscription()
  return sub ? 'subscribed' : 'unsubscribed'
}
