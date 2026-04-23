// OxiRemote service worker — receive Web Push, open deep links.
//
// No precaching, no offline caching. This SW exists solely for push + click
// routing. All fetch requests pass through to the network.

self.addEventListener('install', (event) => {
  self.skipWaiting()
})

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim())
})

self.addEventListener('push', (event) => {
  let data = {}
  try {
    data = event.data ? event.data.json() : {}
  } catch {
    data = { title: 'OxiRemote', body: event.data ? event.data.text() : '' }
  }
  const title = data.title || 'OxiRemote'
  const options = {
    body: data.body || '',
    icon: '/favicon.svg',
    badge: '/favicon.svg',
    tag: data.deep_link || 'oxiremote-notification',
    renotify: true,
    data: { deep_link: data.deep_link || '/' },
  }
  event.waitUntil(self.registration.showNotification(title, options))
})

self.addEventListener('notificationclick', (event) => {
  event.notification.close()

  const raw = (event.notification.data && event.notification.data.deep_link) || '/'
  const deepLink = sanitizeDeepLink(raw)

  event.waitUntil(
    (async () => {
      const allClients = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
      const target = new URL(deepLink, self.location.origin).href

      // Reuse an existing tab if any is on this origin.
      for (const client of allClients) {
        if (new URL(client.url).origin === self.location.origin) {
          await client.focus()
          if ('navigate' in client) {
            try {
              await client.navigate(target)
              return
            } catch {
              // fall through to openWindow
            }
          }
          // SPA navigation via postMessage if navigate() isn't available.
          client.postMessage({ type: 'oxi:deep-link', path: deepLink })
          return
        }
      }

      if (self.clients.openWindow) {
        await self.clients.openWindow(target)
      }
    })(),
  )
})

// Accept only own-origin absolute paths ("/h/..."); reject everything else.
function sanitizeDeepLink(input) {
  if (typeof input !== 'string') return '/'
  if (!input.startsWith('/')) return '/'
  if (input.startsWith('//')) return '/'
  return input
}
