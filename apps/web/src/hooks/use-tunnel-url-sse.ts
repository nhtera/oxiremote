// Proactive tunnel-URL reconnect via SSE.
//
// Mounts a persistent EventSource on `/api/agent/events` and listens for
// `tunnel_url_changed` frames. On receipt it:
//   1. Persists the new URL into localStorage via `storeTunnelBase` so future
//      fetch / WS calls automatically use the rotated URL.
//   2. Dispatches `oxi:tunnel-url-changed` on `window` so WS hooks (terminal,
//      desktop JPEG, desktop H.264) can teardown and reconnect immediately
//      without waiting for their next user action.
//
// This hook is mounted once at the app root (embedded + discovery modes).
// In embedded mode `storeTunnelBase` is a no-op (no active host in discovery
// context) but the window event still benefits terminal / desktop tabs.
//
// Without this hook an idle SPA tab learns about a tunnel rotation only on
// the next API call, causing ~30s of perceived outage until the user retries.

import { useEffect } from 'react'
import { getActiveHost, storeTunnelBase } from '../lib/api-client'
import { isDiscoveryMode } from '../lib/discovery-client'

/** Custom event name — also used by WS hooks to listen for URL changes. */
export const TUNNEL_URL_CHANGED_EVENT = 'oxi:tunnel-url-changed'

export interface TunnelUrlChangedDetail {
  url: string
}

/** Dispatch a browser-level tunnel-URL-changed event so all WS hooks wake up. */
function dispatchTunnelUrlChanged(url: string): void {
  window.dispatchEvent(
    new CustomEvent<TunnelUrlChangedDetail>(TUNNEL_URL_CHANGED_EVENT, {
      detail: { url },
    }),
  )
}

/**
 * Mounts an SSE listener on `/api/agent/events` that handles
 * `tunnel_url_changed` events. Call this once near the app root.
 *
 * In discovery mode the hook also updates `storeTunnelBase` so subsequent
 * cross-origin fetches and WS upgrades use the new tunnel base automatically.
 */
export function useTunnelUrlSse(): void {
  useEffect(() => {
    // `/api/agent/events` is a localhost-only route — only reachable when the
    // SPA is served from the same origin as the agent (embedded mode, or a
    // desktop tab pointed at the local agent). Skip in cross-origin discovery
    // mode where the SPA lives on Cloudflare Pages.
    if (isDiscoveryMode()) return

    const es = new EventSource('/api/agent/events')

    es.onmessage = (msg: MessageEvent<string>) => {
      try {
        const ev = JSON.parse(msg.data) as { type?: string; url?: string }
        if (ev.type !== 'tunnel_url_changed' || typeof ev.url !== 'string') return

        const newUrl = ev.url

        // Persist the new tunnel base so WS-URL builders pick it up without
        // a full page reload. In embedded mode (no discovery) this is a no-op
        // because `getActiveHost()` returns null.
        const hostId = getActiveHost()
        if (hostId && newUrl) {
          storeTunnelBase(hostId, newUrl)
        }

        // Notify all WS hooks regardless of storage outcome.
        dispatchTunnelUrlChanged(newUrl)
      } catch {
        // drop malformed SSE frames
      }
    }

    return () => {
      es.close()
    }
  }, [])
}
